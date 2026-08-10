#!/usr/bin/env python3
"""Derived validation inventory and exact-commit evidence records.

The registry is intentionally small and stdlib-only.  It indexes validation
surfaces that already own their assertions; it is not a parallel test runner.
"""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Iterable


FORMAT = "retinue.validation.result.v1"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SUITE_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
RESULT_KEYS = {
    "format",
    "suite_id",
    "status",
    "revision",
    "manifest_sha256",
    "command",
    "started_at",
    "duration_ms",
    "exit_code",
    "worktree_clean_before",
    "worktree_clean_after",
    "tool_versions",
    "stdout_sha256",
    "stderr_sha256",
}


class ValidationError(RuntimeError):
    """A malformed registry or insufficient evidence record."""


@dataclass(frozen=True)
class Suite:
    identifier: str
    tier: str
    command: tuple[str, ...]
    asset_globs: tuple[str, ...]


@dataclass(frozen=True)
class Exemption:
    path: str
    reason: str
    expires: dt.date


@dataclass(frozen=True)
class Manifest:
    path: Path
    owned_manifest_globs: tuple[str, ...]
    validation_asset_globs: tuple[str, ...]
    suites: tuple[Suite, ...]
    exemptions: tuple[Exemption, ...]

    @property
    def sha256(self) -> str:
        return digest_file(self.path)


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_file(path: Path) -> str:
    return digest_bytes(path.read_bytes())


def command(root: Path, args: list[str], *, check: bool = True) -> str:
    completed = subprocess.run(
        ["git", *args],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if check and completed.returncode:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise ValidationError(f"git {' '.join(args)} failed: {detail}")
    return completed.stdout.strip()


def repo_files(root: Path) -> list[str]:
    # Include unignored additions while developing the registry.  Exact-SHA
    # evidence still refuses a dirty worktree, but the structural check should
    # be able to catch a newly created oracle before it is staged.
    output = command(root, ["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
    return sorted(path for path in output.split("\0") if path)


def git_revision(root: Path) -> str:
    revision = command(root, ["rev-parse", "HEAD"])
    if not SHA_RE.fullmatch(revision):
        raise ValidationError(f"Git did not return a full commit SHA: {revision!r}")
    return revision


def worktree_clean(root: Path) -> bool:
    return not command(root, ["status", "--porcelain=v1", "--untracked-files=all"])


def require_clean_worktree(root: Path) -> None:
    if not worktree_clean(root):
        raise ValidationError(
            "exact-SHA evidence requires a clean worktree; commit or set aside "
            "unrelated changes before recording or aggregating results"
        )


def require_string_list(value: Any, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
        raise ValidationError(f"{label} must be a non-empty list of strings")
    return tuple(value)


def optional_string_list(value: Any, label: str) -> tuple[str, ...]:
    if value is None:
        return ()
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise ValidationError(f"{label} must be a list of non-empty strings")
    return tuple(value)


def load_manifest(path: Path) -> Manifest:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"cannot read {path}: {error}") from error

    if data.get("version") != 1:
        raise ValidationError("manifest version must be 1")
    discovery = data.get("discovery")
    if not isinstance(discovery, dict):
        raise ValidationError("manifest needs a [discovery] table")

    suites_data = data.get("suite")
    if not isinstance(suites_data, list) or not suites_data:
        raise ValidationError("manifest needs at least one [[suite]]")

    suites: list[Suite] = []
    known_ids: set[str] = set()
    for index, item in enumerate(suites_data):
        if not isinstance(item, dict):
            raise ValidationError(f"suite #{index + 1} is not a table")
        identifier = item.get("id")
        tier = item.get("tier")
        if not isinstance(identifier, str) or not SUITE_RE.fullmatch(identifier):
            raise ValidationError(f"suite #{index + 1} has an invalid id")
        if identifier in known_ids:
            raise ValidationError(f"duplicate suite id: {identifier}")
        if tier not in {"pr", "release", "scheduled", "local", "manual"}:
            raise ValidationError(f"suite {identifier} has an invalid tier: {tier!r}")
        known_ids.add(identifier)
        suites.append(
            Suite(
                identifier=identifier,
                tier=tier,
                command=require_string_list(item.get("command"), f"suite {identifier}.command"),
                asset_globs=optional_string_list(item.get("asset_globs"), f"suite {identifier}.asset_globs"),
            )
        )

    exemptions: list[Exemption] = []
    paths: set[str] = set()
    for index, item in enumerate(data.get("asset_exemption", [])):
        if not isinstance(item, dict):
            raise ValidationError(f"asset exemption #{index + 1} is not a table")
        raw_path, reason, raw_expiry = item.get("path"), item.get("reason"), item.get("expires")
        if not isinstance(raw_path, str) or not isinstance(reason, str) or not reason:
            raise ValidationError(f"asset exemption #{index + 1} needs path and reason")
        if raw_path in paths:
            raise ValidationError(f"duplicate asset exemption: {raw_path}")
        try:
            expires = dt.date.fromisoformat(str(raw_expiry))
        except ValueError as error:
            raise ValidationError(f"asset exemption {raw_path} has an invalid expiry") from error
        paths.add(raw_path)
        exemptions.append(Exemption(raw_path, reason, expires))

    return Manifest(
        path=path,
        owned_manifest_globs=require_string_list(
            discovery.get("owned_manifest_globs"), "discovery.owned_manifest_globs"
        ),
        validation_asset_globs=require_string_list(
            discovery.get("validation_asset_globs"), "discovery.validation_asset_globs"
        ),
        suites=tuple(suites),
        exemptions=tuple(exemptions),
    )


def matches(path: str, pattern: str) -> bool:
    """Match POSIX repository paths without platform-specific glob behaviour."""
    return fnmatch.fnmatchcase(path, pattern) or PurePosixPath(path).match(pattern)


def expand(paths: Iterable[str], patterns: Iterable[str]) -> set[str]:
    return {path for path in paths if any(matches(path, pattern) for pattern in patterns)}


def inventory(root: Path, manifest: Manifest) -> dict[str, Any]:
    paths = repo_files(root)
    manifests = sorted(expand(paths, manifest.owned_manifest_globs))
    assets = sorted(expand(paths, manifest.validation_asset_globs))
    diagnostics: list[str] = []

    owners: dict[str, list[str]] = {asset: [] for asset in assets}
    for suite in manifest.suites:
        selected = expand(paths, suite.asset_globs)
        for pattern in suite.asset_globs:
            if not any(matches(path, pattern) for path in paths):
                diagnostics.append(f"suite {suite.identifier} selector matches nothing: {pattern}")
        for asset in selected:
            if asset in owners:
                owners[asset].append(suite.identifier)

    today = dt.datetime.now(dt.UTC).date()
    exemptions = {item.path: item for item in manifest.exemptions}
    for item in manifest.exemptions:
        if item.path not in assets:
            diagnostics.append(f"exemption is not a discovered validation asset: {item.path}")
        if item.expires < today:
            diagnostics.append(f"asset exemption expired on {item.expires.isoformat()}: {item.path}")

    for asset, assigned in owners.items():
        if not assigned and asset not in exemptions:
            diagnostics.append(f"orphan validation asset: {asset}")
        if assigned and asset in exemptions:
            diagnostics.append(f"asset has both suite ownership and an exemption: {asset}")

    return {
        "format": "retinue.validation.inventory.v1",
        "revision": git_revision(root),
        "manifest_sha256": manifest.sha256,
        "cargo_manifests": manifests,
        "assets": [
            {
                "path": asset,
                "suites": sorted(owners[asset]),
                "exemption": (
                    {"reason": exemptions[asset].reason, "expires": exemptions[asset].expires.isoformat()}
                    if asset in exemptions
                    else None
                ),
            }
            for asset in assets
        ],
        "suites": [
            {"id": suite.identifier, "tier": suite.tier, "command": list(suite.command)}
            for suite in manifest.suites
        ],
        "diagnostics": diagnostics,
    }


def suite_by_id(manifest: Manifest, identifier: str) -> Suite:
    for suite in manifest.suites:
        if suite.identifier == identifier:
            return suite
    raise ValidationError(f"unknown suite: {identifier}")


def tool_versions(command_line: tuple[str, ...]) -> dict[str, str]:
    candidates = [command_line[0], "cargo", "rustc", "python3", "python"]
    versions: dict[str, str] = {}
    for executable in dict.fromkeys(candidates):
        if not shutil.which(executable):
            continue
        completed = subprocess.run(
            [executable, "--version"], capture_output=True, text=True, check=False, timeout=10
        )
        output = (completed.stdout or completed.stderr).strip().splitlines()
        versions[executable] = output[0] if output else f"exit {completed.returncode}"
    return versions


def timestamp() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def evidence_output(root: Path, output: Path) -> Path:
    """Resolve a record path without allowing the recorder to dirty its source."""
    resolved = output.resolve() if output.is_absolute() else (root / output).resolve()
    try:
        relative = resolved.relative_to(root)
    except ValueError:
        return resolved

    ignored = subprocess.run(
        ["git", "check-ignore", "-q", "--no-index", "--", relative.as_posix()],
        cwd=root,
        check=False,
    ).returncode == 0
    if not ignored:
        raise ValidationError(
            "result output inside the worktree must be Git-ignored; use "
            "validation/results/<commit>/... or an external evidence directory"
        )
    return resolved


def record_suite(root: Path, manifest: Manifest, suite: Suite, output: Path) -> int:
    require_clean_worktree(root)
    output = evidence_output(root, output)
    revision = git_revision(root)
    started_at = timestamp()
    start = time.monotonic()
    completed = subprocess.run(
        suite.command,
        cwd=root,
        capture_output=True,
        check=False,
    )
    duration_ms = round((time.monotonic() - start) * 1000)
    clean_after = worktree_clean(root)
    result = {
        "format": FORMAT,
        "suite_id": suite.identifier,
        "status": "passed" if completed.returncode == 0 else "failed",
        "revision": revision,
        "manifest_sha256": manifest.sha256,
        "command": list(suite.command),
        "started_at": started_at,
        "duration_ms": duration_ms,
        "exit_code": completed.returncode,
        "worktree_clean_before": True,
        "worktree_clean_after": clean_after,
        "tool_versions": tool_versions(suite.command),
        "stdout_sha256": digest_bytes(completed.stdout),
        "stderr_sha256": digest_bytes(completed.stderr),
    }
    write_json(output, result)
    if not clean_after or not worktree_clean(root):
        raise ValidationError("suite dirtied the worktree; its result record is invalid")
    return completed.returncode


def validate_result(data: Any, suite: Suite, revision: str, manifest_sha256: str) -> None:
    if not isinstance(data, dict):
        raise ValidationError("result is not an object")
    if set(data) != RESULT_KEYS:
        missing = sorted(RESULT_KEYS - set(data))
        extra = sorted(set(data) - RESULT_KEYS)
        raise ValidationError(f"result schema mismatch; missing={missing}, extra={extra}")
    if data["format"] != FORMAT:
        raise ValidationError("result has the wrong format")
    if data["suite_id"] != suite.identifier:
        raise ValidationError("result suite id does not match its declared suite")
    if data["status"] != "passed" or data["exit_code"] != 0:
        raise ValidationError(f"suite {suite.identifier} did not pass")
    if not isinstance(data["revision"], str) or data["revision"] != revision or not SHA_RE.fullmatch(data["revision"]):
        raise ValidationError(f"suite {suite.identifier} is not evidence for {revision}")
    if (
        not isinstance(data["manifest_sha256"], str)
        or data["manifest_sha256"] != manifest_sha256
        or not re.fullmatch(r"[0-9a-f]{64}", data["manifest_sha256"])
    ):
        raise ValidationError(f"suite {suite.identifier} used a different validation manifest")
    if data["command"] != list(suite.command) or not all(isinstance(item, str) for item in data["command"]):
        raise ValidationError(f"suite {suite.identifier} did not run its declared command")
    if not isinstance(data["started_at"], str) or not data["started_at"].endswith("Z"):
        raise ValidationError(f"suite {suite.identifier} has no UTC timestamp")
    try:
        parsed_time = dt.datetime.fromisoformat(data["started_at"].replace("Z", "+00:00"))
    except ValueError as error:
        raise ValidationError(f"suite {suite.identifier} has an invalid timestamp") from error
    if parsed_time.tzinfo != dt.UTC:
        raise ValidationError(f"suite {suite.identifier} timestamp is not UTC")
    if type(data["duration_ms"]) is not int or data["duration_ms"] < 0:
        raise ValidationError(f"suite {suite.identifier} has an invalid duration")
    if type(data["exit_code"]) is not int:
        raise ValidationError(f"suite {suite.identifier} has an invalid exit code")
    if not isinstance(data["tool_versions"], dict) or not all(
        isinstance(key, str) and isinstance(value, str) for key, value in data["tool_versions"].items()
    ):
        raise ValidationError(f"suite {suite.identifier} has invalid tool versions")
    if data["worktree_clean_before"] is not True or data["worktree_clean_after"] is not True:
        raise ValidationError(f"suite {suite.identifier} was not recorded in a clean worktree")
    for key in ("stdout_sha256", "stderr_sha256"):
        if not isinstance(data[key], str) or not re.fullmatch(r"[0-9a-f]{64}", data[key]):
            raise ValidationError(f"suite {suite.identifier} has an invalid {key}")


def verify_results(root: Path, manifest: Manifest, results: Path, revision: str, tiers: set[str]) -> None:
    require_clean_worktree(root)
    actual = git_revision(root)
    if revision != actual:
        raise ValidationError(f"requested revision {revision} is not checked out (HEAD is {actual})")
    if not results.is_dir():
        raise ValidationError(f"results directory does not exist: {results}")

    declared = {suite.identifier: suite for suite in manifest.suites if suite.tier in tiers}
    seen: set[str] = set()
    for path in sorted(results.rglob("*.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ValidationError(f"cannot read result {path}: {error}") from error
        identifier = data.get("suite_id") if isinstance(data, dict) else None
        if identifier not in declared:
            raise ValidationError(f"result is not required by selected tiers: {path}")
        if identifier in seen:
            raise ValidationError(f"duplicate result for suite {identifier}")
        validate_result(data, declared[identifier], revision, manifest.sha256)
        seen.add(identifier)

    missing = sorted(set(declared) - seen)
    if missing:
        raise ValidationError(f"missing exact-SHA result records: {', '.join(missing)}")


def init_test_repository(root: Path) -> tuple[Manifest, Suite, Path]:
    (root / "validation").mkdir(parents=True)
    (root / "crates/demo/oracle").mkdir(parents=True)
    (root / "validation/tool.py").write_text("# tool\n", encoding="utf-8")
    (root / "crates/demo/oracle/gate.py").write_text("# oracle\n", encoding="utf-8")
    (root / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")
    (root / ".gitignore").write_text("validation/results/\n", encoding="utf-8")
    probe_command = json.dumps([sys.executable, "-c", "print('probe')"])
    (root / "validation/manifest.toml").write_text(
        "\n".join(
            [
                "version = 1",
                "[discovery]",
                'owned_manifest_globs = ["Cargo.toml", "crates/*/Cargo.toml"]',
                'validation_asset_globs = ["validation/*.py", "crates/*/oracle/*.py"]',
                "[[suite]]",
                'id = "probe"',
                'tier = "pr"',
                f"command = {probe_command}",
                'asset_globs = ["validation/*.py"]',
                "[[suite]]",
                'id = "oracle"',
                'tier = "local"',
                f"command = {probe_command}",
                'asset_globs = ["crates/*/oracle/*.py"]',
                "",
            ]
        ),
        encoding="utf-8",
    )
    for args in (["init"], ["config", "user.email", "validation@example.invalid"], ["config", "user.name", "Validation"], ["add", "."], ["commit", "-m", "fixture"]):
        command(root, list(args))
    manifest = load_manifest(root / "validation/manifest.toml")
    return manifest, suite_by_id(manifest, "probe"), root / "validation/results/probe.json"


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="retinue-validation-") as directory:
        root = Path(directory)
        manifest, suite, output = init_test_repository(root)
        report = inventory(root, manifest)
        if report["diagnostics"]:
            raise ValidationError(f"self-test inventory diagnostics: {report['diagnostics']}")
        if report["cargo_manifests"] != ["Cargo.toml"]:
            raise ValidationError("self-test failed to inventory Cargo manifests")
        try:
            record_suite(root, manifest, suite, root / "unignored-result.json")
        except ValidationError as error:
            if "Git-ignored" not in str(error):
                raise
        else:
            raise ValidationError("self-test allowed a result to dirty the worktree")
        if record_suite(root, manifest, suite, output) != 0:
            raise ValidationError("self-test command failed")
        record = json.loads(output.read_text(encoding="utf-8"))
        revision = git_revision(root)
        validate_result(record, suite, revision, manifest.sha256)
        verify_results(root, manifest, output.parent, revision, {"pr"})
        record["revision"] = "0" * 40
        write_json(output, record)
        try:
            verify_results(root, manifest, output.parent, revision, {"pr"})
        except ValidationError:
            return
        raise ValidationError("self-test accepted a result from the wrong commit")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="action", required=True)
    commands.add_parser("verify", help="check the derived validation inventory")
    record = commands.add_parser("record", help="run one suite and create an exact-SHA result record")
    record.add_argument("suite")
    record.add_argument("--output", required=True, type=Path)
    aggregate = commands.add_parser("verify-results", help="validate a release evidence directory")
    aggregate.add_argument("--results", required=True, type=Path)
    aggregate.add_argument("--revision", required=True)
    aggregate.add_argument("--tiers", default="pr,release")
    commands.add_parser("self-test", help="exercise the tool in a temporary Git repository")
    return result


def main() -> int:
    args = parser().parse_args()
    root = Path(__file__).resolve().parents[1]
    try:
        if args.action == "self-test":
            self_test()
            print("validation self-test: passed")
            return 0

        manifest = load_manifest(root / "validation/manifest.toml")
        if args.action == "verify":
            report = inventory(root, manifest)
            if report["diagnostics"]:
                raise ValidationError("\n".join(report["diagnostics"]))
            print(
                "validation inventory: "
                f"{len(report['cargo_manifests'])} Cargo manifests, "
                f"{len(report['assets'])} validation assets, "
                f"{len(report['suites'])} suites at {report['revision']}"
            )
            return 0
        if args.action == "record":
            return record_suite(root, manifest, suite_by_id(manifest, args.suite), args.output)
        if args.action == "verify-results":
            tiers = {item.strip() for item in args.tiers.split(",") if item.strip()}
            allowed = {"pr", "release", "scheduled", "local", "manual"}
            if not tiers or not tiers <= allowed:
                raise ValidationError("--tiers must contain only pr, release, scheduled, local, or manual")
            verify_results(root, manifest, args.results, args.revision, tiers)
            print(f"validation results: exact-SHA evidence complete for {args.revision}")
            return 0
        raise AssertionError(f"unhandled action: {args.action}")
    except ValidationError as error:
        print(f"validation error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
