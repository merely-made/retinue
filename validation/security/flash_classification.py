#!/usr/bin/env python3
# Design derived from Prns, github.com/KenAKAFrosty/Prns, Copyright (c) 2026 The Prns
# Authors, MIT OR Apache-2.0 (MIT elected). The discipline is theirs; no Prns text is
# copied here. See THIRD_PARTY_NOTICES.md and
# design_docs/2026-08-10_prns_donor_ledger.md.
"""FS5: enumerate what a seized node yields, and refuse an unclassified addition.

The security posture states a seizure paragraph as a design target, and adds a standing rule
that every settings-record field is classified public / pseudonymous / forbidden at the
moment it is added.  A rule with no check is a rule that holds until someone is in a hurry.

This audit reads the persisted schema out of the Rust source, requires a classification for
every field it finds, refuses a classification for a field that no longer exists, and fails
on any persisted name matching a forbidden pattern.  Its report is the flash inventory the
seizure paragraph is compared against.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

CLASSIFICATIONS = ("public", "pseudonymous", "forbidden")


class PolicyError(RuntimeError):
    """A flash-classification contract was not met."""


@dataclass(frozen=True)
class Field:
    name: str
    type_name: str


def struct_fields(source: str, struct: str) -> tuple[Field, ...]:
    """Extract the public fields of one struct.

    Deliberately narrow: it finds the named struct's brace-delimited body and reads
    `pub name: Type` lines out of it, skipping comments and attributes.  A parser that
    understood all of Rust would be a bigger thing to trust than the rule it enforces, so the
    self-test below pins exactly what this does and does not read.
    """
    match = re.search(rf"(?m)^\s*pub struct {re.escape(struct)}\s*\{{", source)
    if not match:
        raise PolicyError(f"no `pub struct {struct}` in the schema source")

    depth = 0
    start = match.end() - 1
    end = None
    for index in range(start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                end = index
                break
    if end is None:
        raise PolicyError(f"`pub struct {struct}` is not closed")

    fields = []
    for line in source[start + 1 : end].splitlines():
        line = line.strip()
        if not line or line.startswith(("//", "#[", "/*", "*")):
            continue
        field = re.match(r"pub\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+?),?$", line)
        if field:
            fields.append(Field(field.group(1), field.group(2).rstrip(",").strip()))
    return tuple(fields)


def load_policy(path: Path) -> dict[str, Any]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise PolicyError(f"cannot read {path.name}: {error}") from error


def forbidden_hits(root: Path, patterns: tuple[str, ...], globs: tuple[str, ...]) -> list[str]:
    """Any persisted field name containing a forbidden substring.

    Scans field-declaration lines rather than whole files, so a comment naming a PSK to
    explain why one is absent does not fail the build that keeps it absent.
    """
    hits = []
    for glob in globs:
        for path in sorted(root.glob(glob)):
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                declaration = re.match(
                    r"\s*(?:pub\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*[^=]+,?\s*$", line
                )
                if not declaration:
                    continue
                name = declaration.group(1).lower()
                for pattern in patterns:
                    if pattern in name:
                        rel = path.relative_to(root).as_posix()
                        hits.append(f"{rel}:{number}: persisted field `{name}` matches forbidden pattern `{pattern}`")
    return hits


def classify(fields: tuple[Field, ...], policy: dict[str, Any], policy_name: str) -> tuple[list[dict[str, Any]], list[str]]:
    """Match the schema against the policy. Pure, so the self-test can drive it."""
    classified: dict[str, dict[str, Any]] = {}
    diagnostics: list[str] = []
    for entry in policy.get("field", []):
        name = entry["name"]
        if name in classified:
            diagnostics.append(f"field `{name}` is classified twice")
        if entry["classification"] not in CLASSIFICATIONS:
            diagnostics.append(
                f"field `{name}` has classification `{entry['classification']}`; "
                f"expected one of {', '.join(CLASSIFICATIONS)}"
            )
        if not entry.get("reason", "").strip():
            diagnostics.append(f"field `{name}` is classified without a reason")
        classified[name] = entry

    present = {field.name for field in fields}
    for field in fields:
        entry = classified.get(field.name)
        if entry is None:
            diagnostics.append(
                f"settings-record field `{field.name}: {field.type_name}` has no classification. "
                f"Classify it in {policy_name} before it ships."
            )
        elif entry["classification"] == "forbidden":
            diagnostics.append(
                f"settings-record field `{field.name}` is classified forbidden and is present "
                f"in the schema. It must not live on a pole: {entry['reason'].strip()}"
            )
    for name, entry in classified.items():
        if name not in present and entry["classification"] != "forbidden":
            diagnostics.append(
                f"`{name}` is classified but is no longer a field of "
                f"{policy.get('schema_struct', 'the schema')}. Remove the stale entry."
            )

    inventory = [
        {
            "name": field.name,
            "type": field.type_name,
            "classification": classified.get(field.name, {}).get("classification", "UNCLASSIFIED"),
        }
        for field in fields
    ]
    return inventory, diagnostics


def audit(root: Path) -> dict[str, Any]:
    policy_path = root / "validation" / "security" / "flash-policy.toml"
    policy = load_policy(policy_path)

    schema_path = root / policy["schema_source"]
    if not schema_path.is_file():
        raise PolicyError(f"schema source {policy['schema_source']} does not exist")
    fields = struct_fields(schema_path.read_text(encoding="utf-8"), policy["schema_struct"])

    inventory, diagnostics = classify(fields, policy, policy_path.name)

    forbidden = policy.get("forbidden", {})
    diagnostics.extend(
        forbidden_hits(
            root,
            tuple(forbidden.get("name_patterns", ())),
            tuple(forbidden.get("scan_globs", ())),
        )
    )

    adjacent = [
        {
            "name": entry["name"],
            "location": entry["location"],
            "classification": entry["classification"],
        }
        for entry in policy.get("adjacent", [])
    ]
    return {"inventory": inventory, "adjacent": adjacent, "diagnostics": diagnostics}


def self_test() -> None:
    """Pin the field reader, because the whole audit rests on what it does and does not see."""
    source = """
        /// A doc comment mentioning pub ghost: u8 which must not be read.
        #[derive(Debug)]
        pub struct Settings {
            /// The device identity.
            pub identity: [u8; IDENTITY_LEN],
            // pub commented_out: u8,
            pub channel: Channel,
            pub region: crate::region::Region,
            private_field: u8,
        }

        pub struct Other {
            pub decoy: u8,
        }
    """
    fields = struct_fields(source, "Settings")
    assert [f.name for f in fields] == ["identity", "channel", "region"], fields
    assert fields[0].type_name == "[u8; IDENTITY_LEN]", fields[0]
    assert [f.name for f in struct_fields(source, "Other")] == ["decoy"]
    try:
        struct_fields(source, "Missing")
    except PolicyError:
        pass
    else:
        raise AssertionError("a missing struct must be an error, not an empty schema")

    # An audit nobody has watched fail is an audit nobody should trust. Each case below is a
    # way this check is supposed to stop a change, driven against the real decision code.
    good = {
        "field": [
            {"name": "identity", "classification": "pseudonymous", "reason": "the relay keypair"},
            {"name": "channel", "classification": "public", "reason": "observable on air"},
            {"name": "region", "classification": "public", "reason": "observable on air"},
        ]
    }
    _, clean = classify(fields, good, "policy.toml")
    assert clean == [], clean

    def diagnostics_for(policy: dict[str, Any], schema: tuple[Field, ...] = fields) -> str:
        return "\n".join(classify(schema, policy, "policy.toml")[1])

    # A field added to the schema with no classification.
    added = fields + (Field("wifi_ssid", "heapless::String<32>"),)
    assert "has no classification" in diagnostics_for(good, added)

    # A field that policy says must never be here, present anyway.
    rejected = {"field": good["field"] + [{"name": "psk", "classification": "forbidden", "reason": "host credential"}]}
    assert "must not live on a pole" in diagnostics_for(rejected, fields + (Field("psk", "[u8; 32]"),))

    # A classification left behind after its field was removed.
    assert "no longer a field" in diagnostics_for(good, fields[:-1])

    # A classification with no reason, and one that is not a classification at all.
    reasonless = {"field": [dict(good["field"][0], reason="  ")] + good["field"][1:]}
    assert "without a reason" in diagnostics_for(reasonless)
    invented = {"field": [dict(good["field"][0], classification="mostly harmless")] + good["field"][1:]}
    assert "expected one of" in diagnostics_for(invented)

    # And the forbidden-name scan, against a line that looks exactly like a real field.
    import tempfile

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        (root / "src").mkdir()
        (root / "src" / "store.rs").write_text(
            "// A comment naming the psk we deliberately do not store.\n"
            "pub struct Settings {\n"
            "    pub identity: [u8; 64],\n"
            "    pub wifi_psk: [u8; 32],\n"
            "}\n",
            encoding="utf-8",
        )
        hits = forbidden_hits(root, ("psk",), ("src/*.rs",))
        assert len(hits) == 1, hits
        assert "wifi_psk" in hits[0], hits
        assert forbidden_hits(root, ("ssid",), ("src/*.rs",)) == []


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="write the inventory as JSON")
    parser.add_argument("--self-test", action="store_true", help="exercise the field reader")
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
            print("flash classification self-test: passed")
            return 0
        report = audit(Path(__file__).resolve().parents[2])
        if report["diagnostics"]:
            raise PolicyError("\n".join(report["diagnostics"]))
        if args.json:
            print(json.dumps(report, indent=2, sort_keys=True))
        else:
            print(
                f"flash classification: {len(report['inventory'])} settings fields and "
                f"{len(report['adjacent'])} adjacent records, all classified"
            )
            for item in report["inventory"] + report["adjacent"]:
                print(f"  {item['classification']:>13}  {item['name']}")
        return 0
    except PolicyError as error:
        print(f"flash classification error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
