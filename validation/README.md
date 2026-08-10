# Retinue validation registry

`validation/manifest.toml` is a derived inventory of Retinue's cross-boundary
validation surfaces. It names commands and their owner, but does not duplicate
test assertions from Cargo tests, live oracles, capture scripts, or physical
receipts.

Run the structural check:

```sh
python3 validation/run.py verify
```

It discovers owned Cargo manifests and validation scripts from Git, checks that
every discovered script belongs to a suite or a dated exemption, and rejects
stale selectors and exemptions. It is safe to run in a dirty worktree because
it does not create evidence.

Record a command only from a clean worktree:

```sh
python3 validation/run.py record host-workspace \
  --output validation/results/<40-char-commit>/host-workspace.json
```

The record stores the exact commit, manifest digest, resolved command, tool
versions, elapsed time, exit code, and hashes of stdout and stderr. Generated
records are ignored deliberately: evidence is valid only for the named clean
commit, not for a later working tree.

Validate a release evidence directory against one commit:

```sh
python3 validation/run.py verify-results \
  --results validation/results/<40-char-commit> \
  --revision <40-char-commit> \
  --tiers pr,release
```

`self-test` creates a temporary Git repository and exercises inventory and
exact-SHA record validation without using a board, venv, or Cargo build.

The live oracle commands are intentionally local. Use the pinned virtual
environment and capture procedures in their owner directories when running
them on Windows; the command shown in the manifest is the CI/Linux spelling.
