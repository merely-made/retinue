#!/usr/bin/env python3
# Design derived from Prns, github.com/KenAKAFrosty/Prns, Copyright (c) 2026 The Prns
# Authors, MIT OR Apache-2.0 (MIT elected). The discipline is theirs; no Prns text is
# copied here. See THIRD_PARTY_NOTICES.md and
# design_docs/2026-08-10_prns_donor_ledger.md.
"""Enforce Retinue's first-party unsafe-code policy without scanning dependencies."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


class AuditError(RuntimeError):
    """An unsafe-policy contract was not met."""


@dataclass(frozen=True)
class ExceptionRule:
    path: str
    expected_unsafe: int
    reason: str
    review_by: dt.date


def git_files(root: Path) -> list[str]:
    completed = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=root,
        capture_output=True,
        check=False,
    )
    if completed.returncode:
        raise AuditError(completed.stderr.decode().strip() or "git ls-files failed")
    return sorted(path.decode() for path in completed.stdout.split(b"\0") if path)


def load_policy(path: Path) -> tuple[tuple[str, ...], str, str, tuple[ExceptionRule, ...]]:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise AuditError(f"cannot read policy: {error}") from error
    if data.get("version") != 1 or not isinstance(data.get("policy"), dict):
        raise AuditError("unsafe policy must have version = 1 and a [policy] table")
    policy: dict[str, Any] = data["policy"]
    roots = policy.get("source_roots")
    safe_lint = policy.get("safe_crate_lint")
    exception_lint = policy.get("exception_crate_lint")
    if not isinstance(roots, list) or not all(isinstance(root, str) and root for root in roots):
        raise AuditError("policy.source_roots must be a non-empty string list")
    if not isinstance(safe_lint, str) or not isinstance(exception_lint, str):
        raise AuditError("policy must name safe_crate_lint and exception_crate_lint")

    rules: list[ExceptionRule] = []
    known: set[str] = set()
    for item in data.get("exception", []):
        if not isinstance(item, dict):
            raise AuditError("unsafe exception is not a table")
        raw_path, count, reason, review = (
            item.get("path"),
            item.get("expected_unsafe"),
            item.get("reason"),
            item.get("review_by"),
        )
        if (
            not isinstance(raw_path, str)
            or type(count) is not int
            or count <= 0
            or not isinstance(reason, str)
            or not reason
            or raw_path in known
        ):
            raise AuditError("every unsafe exception needs one path, positive count, and reason")
        try:
            review_by = dt.date.fromisoformat(str(review))
        except ValueError as error:
            raise AuditError(f"unsafe exception {raw_path} has an invalid review date") from error
        known.add(raw_path)
        rules.append(ExceptionRule(raw_path, count, reason, review_by))
    return tuple(roots), safe_lint, exception_lint, tuple(rules)


def unsafe_lines(source: str) -> list[int]:
    """Return lexical `unsafe` tokens, skipping comments and quoted literals.

    This is intentionally a narrow lexer rather than a regex: policy counts must not drift
    because a prose comment, a test string, or a raw literal contains the word "unsafe".
    """
    found: list[int] = []
    index = 0
    line = 1
    length = len(source)
    while index < length:
        if source.startswith("//", index):
            end = source.find("\n", index)
            if end < 0:
                break
            index = end
            continue
        if source.startswith("/*", index):
            depth = 1
            index += 2
            while index < length and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    line += source[index] == "\n"
                    index += 1
            continue

        raw = re.match(r"(?:br|r)(?P<hashes>#{0,255})\"", source[index:])
        if raw:
            marker = '"' + raw.group("hashes")
            index += raw.end()
            end = source.find(marker, index)
            if end < 0:
                line += source[index:].count("\n")
                break
            line += source[index : end + len(marker)].count("\n")
            index = end + len(marker)
            continue
        if source[index] == '"' or source.startswith('b"', index):
            if source.startswith('b"', index):
                index += 1
            index += 1
            while index < length:
                if source[index] == "\\":
                    index += 2
                    continue
                if source[index] == '"':
                    index += 1
                    break
                line += source[index] == "\n"
                index += 1
            continue
        if source[index] == "'":
            # A character literal may contain an escaped quote; a lifetime has no closing
            # quote and is harmless because it cannot spell the full keyword.
            closing = index + 1
            while closing < length:
                if source[closing] == "\\":
                    closing += 2
                    continue
                if source[closing] == "'":
                    line += source[index : closing + 1].count("\n")
                    index = closing + 1
                    break
                if source[closing] in "\n;":
                    index += 1
                    break
                closing += 1
            else:
                index += 1
            continue
        if source[index].isalpha() or source[index] == "_":
            end = index + 1
            while end < length and (source[end].isalnum() or source[end] == "_"):
                end += 1
            if source[index:end] == "unsafe":
                found.append(line)
            index = end
            continue
        line += source[index] == "\n"
        index += 1
    return found


def crate_roots(paths: list[str], roots: tuple[str, ...]) -> list[str]:
    manifests = [
        path
        for path in paths
        if path.endswith("/Cargo.toml") and any(path.startswith(f"{root}/") for root in roots)
    ]
    crate_sources: list[str] = []
    for manifest in manifests:
        base = manifest.removesuffix("Cargo.toml")
        candidates = [f"{base}src/lib.rs", f"{base}src/main.rs"]
        sources = [candidate for candidate in candidates if candidate in paths]
        if not sources:
            raise AuditError(f"first-party package has no lib.rs or main.rs: {manifest}")
        crate_sources.extend(sources)
    return sorted(crate_sources)


def audit(root: Path) -> dict[str, Any]:
    paths = git_files(root)
    policy_path = root / "validation/security/unsafe-policy.toml"
    roots, safe_lint, exception_lint, rules = load_policy(policy_path)
    source_files = [
        path
        for path in paths
        if path.endswith(".rs") and any(path.startswith(f"{prefix}/") for prefix in roots)
    ]
    counts = {path: unsafe_lines((root / path).read_text(encoding="utf-8")) for path in source_files}
    present = {path: lines for path, lines in counts.items() if lines}
    rules_by_path = {rule.path: rule for rule in rules}
    diagnostics: list[str] = []
    today = dt.datetime.now(dt.UTC).date()

    for path, lines in present.items():
        rule = rules_by_path.get(path)
        if rule is None:
            diagnostics.append(f"unapproved unsafe token(s) in {path}: lines {lines}")
        elif len(lines) != rule.expected_unsafe:
            diagnostics.append(
                f"unsafe count drift in {path}: expected {rule.expected_unsafe}, found {len(lines)}"
            )
    for rule in rules:
        if rule.path not in present:
            diagnostics.append(f"unsafe exception has no current unsafe token: {rule.path}")
        if rule.review_by < today:
            diagnostics.append(f"unsafe exception review expired on {rule.review_by}: {rule.path}")

    for crate_root in crate_roots(paths, roots):
        package = crate_root.removesuffix("src/lib.rs").removesuffix("src/main.rs")
        package_unsafe = any(path.startswith(package) for path in present)
        source = (root / crate_root).read_text(encoding="utf-8")
        required = exception_lint if package_unsafe else safe_lint
        if required not in source:
            diagnostics.append(f"{crate_root} must declare {required}")

    return {
        "format": "retinue.unsafe-audit.v1",
        "unsafe_files": [
            {"path": path, "count": len(lines), "lines": lines} for path, lines in sorted(present.items())
        ],
        "crate_roots": crate_roots(paths, roots),
        "diagnostics": diagnostics,
    }


def self_test() -> None:
    source = '''// unsafe\nlet ordinary = "unsafe";\n/* unsafe */\nunsafe fn one() {}\n'''
    if unsafe_lines(source) != [4]:
        raise AuditError("lexer self-test did not ignore comments and strings")
    if unsafe_lines('r#"unsafe"#\n#[unsafe(link_section = ".x")]') != [2]:
        raise AuditError("lexer self-test did not distinguish raw strings and attributes")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="write the audit report as JSON")
    parser.add_argument("--self-test", action="store_true", help="exercise the narrow Rust lexer")
    args = parser.parse_args()
    try:
        if args.self_test:
            self_test()
            print("unsafe audit self-test: passed")
            return 0
        report = audit(Path(__file__).resolve().parents[2])
        if report["diagnostics"]:
            raise AuditError("\n".join(report["diagnostics"]))
        if args.json:
            print(json.dumps(report, indent=2, sort_keys=True))
        else:
            total = sum(item["count"] for item in report["unsafe_files"])
            print(
                f"unsafe audit: {total} approved tokens in {len(report['unsafe_files'])} files; "
                f"{len(report['crate_roots'])} first-party crate roots checked"
            )
        return 0
    except AuditError as error:
        print(f"unsafe audit error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
