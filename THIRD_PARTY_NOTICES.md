# Third-party notices

Retinue is licensed under the Mozilla Public License, Version 2.0 (see
`LICENSE`), including the firmware. This file aggregates the third-party work
this repository derives from, and points at the per-crate notices that carry
the full license texts.

An itemized account of what was taken, in what form, and what it means for
evidence labelling lives in
[`design_docs/2026-08-10_prns_donor_ledger.md`](design_docs/2026-08-10_prns_donor_ledger.md).

## Prns (MIT OR Apache-2.0, MIT elected)

<https://github.com/KenAKAFrosty/Prns> at commit
`72b6b30d27cac910ce20d370e1dc711fe9b95955` (v0.3.4), Copyright (c) 2026 The
Prns Authors.

- **`crates/retinue/src/artifact.rs`, `crates/retinue/src/msgpack.rs`** —
  the RNS signed-artifact envelope layout was read from
  `prns-core/src/identity/signed_artifact.rs` and reimplemented. Full notice
  and license text: [`crates/retinue/NOTICE`](crates/retinue/NOTICE).
- **`crates/retinue/tests/signed_artifact.rs`** — quotes one 224-byte hex
  constant from Prns's tests, as evidence about Prns rather than as an
  implementation input. This is the only verbatim copy of Prns text in the
  tree.
- **`validation/`, `fuzz/`** — the validation registry, evidence discipline,
  tier split, unsafe-policy audit shape, and whole-ingest fuzzing shape were
  reimplemented from Prns's validation hub. Measured line overlap with the
  corresponding Prns files is 3.5% to 8.1% and consists of import statements
  and generic control flow, so no Prns text was copied. Attribution is for the
  design.

## MeshCore (MIT)

`crates/tucket` ports MeshCore wire formats, cryptographic construction,
dedup, and forwarding mechanics. Full notice and license text:
[`crates/tucket/NOTICE`](crates/tucket/NOTICE). How that implementation was
built: [`crates/sennet/PROVENANCE.md`](crates/sennet/PROVENANCE.md).

## lora-phy (MIT OR Apache-2.0)

`vendor/lora-phy` is a vendored third-party fork and keeps its own terms. See
the license files in that directory.

## Reticulum

The Reticulum protocol specification and manual are public domain. The Python
reference implementation is used strictly as a black-box interoperability
oracle, run and observed; its source has never been read. RNS is not
affiliated with this project. `crates/retinue/oracle/RETICULUM_LICENSE` records
the reference implementation's terms.
