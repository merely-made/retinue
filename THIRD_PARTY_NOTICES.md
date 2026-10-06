# Third-party notices

The Retinue workspace is licensed under the Mozilla Public License, Version 2.0
(see `LICENSE`). The exceptions are `crates/retinue` and `crates/outrider`,
which are under the Reticulum License from 2026-10-05 (see each crate's
`LICENSE` and the [Reticulum](#reticulum) section). This file aggregates the third-party work
this repository derives from or uses as a reference, and points at notices carrying
the full license texts.

An itemized account of what was taken, in what form, and what it means for
evidence labelling lives in
[`design_docs/2026-08-10_prns_donor_ledger.md`](design_docs/2026-08-10_prns_donor_ledger.md).

## Prns (historically MIT elected; reference withdrawn)

As of September 26, Prns is withdrawn as an implementation donor and trusted
independent reference over unresolved provenance concerns raised by the owner.
The following notices document retained material, not endorsement or a fresh
assurance of upstream rights. The donor ledger records reassessment scope.
No new Prns-derived work is authorized.

<https://github.com/KenAKAFrosty/Prns> at commit
`72b6b30d27cac910ce20d370e1dc711fe9b95955` (v0.3.4), Copyright (c) 2026 The
Prns Authors.

- **`crates/retinue/src/artifact.rs`, `crates/retinue/src/msgpack.rs`** —
  the RNS signed-artifact envelope layout was read from
  `prns-core/src/identity/signed_artifact.rs` and reimplemented. Full notice
  and license text: [`crates/retinue/NOTICE`](crates/retinue/NOTICE).
- **Historical signed-artifact tests** quoted a 224-byte Prns constant.
  That comparison and donor-specific active fixture inputs were retired on
  September 26. Old captures remain in `testing/receipts/prns-retirement`;
  current captures use project-selected inputs and separately identified RNS 1.5.2
  and 1.5.4 runs. The [1.5.4 receipt](testing/receipts/rns-1.5.4-signed-artifact/README.md)
  preserves the version comparison without changing the historical provenance.
- **`validation/`, `fuzz/`** — the validation registry, evidence discipline,
  tier split, unsafe-policy audit shape, and whole-ingest fuzzing shape were
  reimplemented from Prns's validation hub. Measured line overlap with the
  corresponding Prns files is 3.5% to 8.1% and consists of import statements
  and generic control flow, so no Prns text was copied. Attribution is for the
  design.
- **Historical `crates/retinue/src/announce_admission.rs`** used Prns-influenced
  state-machine design. September 26 replaces that implementation with virtual
  arrival budgets under Retinue's own documented policy. Public settings and
  their defaults remain compatible; historical attribution is retained.
- **`testing/receipts/prns-retirement/hopspot-v4-0.3.4/`** — archives Prns's official
  Hopspot v0.3.4 release binaries unmodified (application, bootloader,
  partition table, signed manifests) for historical receipts. They are excluded
  from active catalogs and stage assembly; the repository still contains them. Prns's terms (MIT elected) apply;
  full license text: [`crates/retinue/NOTICE`](crates/retinue/NOTICE).

## MeshCore (MIT)

`crates/tucket` ports MeshCore wire formats, cryptographic construction,
dedup, and forwarding mechanics. Full notice and license text:
[`crates/tucket/NOTICE`](crates/tucket/NOTICE). How that implementation was
built: [`crates/sennet/PROVENANCE.md`](crates/sennet/PROVENANCE.md).

## lora-phy (MIT OR Apache-2.0)

`vendor/lora-phy` is a vendored third-party fork and keeps its own terms. See
the license files in that directory.

## Meshtastic firmware (GPL-3.0)

`firmware/packages/meshtastic-t114-2.7.26.54e0d8d/` redistributes the official
unmodified Meshtastic release binary
`firmware-heltec-mesh-node-t114-2.7.26.54e0d8d.uf2`, Copyright Meshtastic LLC
and the Meshtastic firmware contributors, licensed GPL-3.0. It is carried as a
restore package for boards this project tests against, not linked into any
Retinue artifact; the license gate in CI exists precisely to keep GPL code out
of the linked firmware. The full license text accompanies the binary as
`LICENSE.GPL-3.0` in the same directory. Complete corresponding source for
exactly this build:
<https://github.com/meshtastic/firmware/tree/54e0d8d> (release v2.7.26).

## Reticulum

**Reticulum License, adopted 2026-10-05** for `crates/retinue` and
`crates/outrider`. The decision record is
[`design_docs/2026-10-05_reticulum_license_adoption.md`](design_docs/2026-10-05_reticulum_license_adoption.md).
The RNS and LXMF reference implementations (Copyright (c) 2016-2026 and
2020-2025 Mark Qvist) and the Reticulum manual may now be read and adapted
into those two crates. Each crate's `LICENSE` carries the upstream text and
notices, and each crate's `NOTICE` lists what has been adapted. Releases
published through 0.2.0 were MPL-2.0 and keep those terms.

What this means in practice:

- **Adapted code stays in those two crates.** No RNS- or LXMF-derived code
  enters the MPL-2.0 crates, the apps, or the firmware crates.
- **Linking carries the terms.** Anything that links retinue or outrider ships
  Reticulum-licensed code. That includes the apps, the firmware images, and
  downstream projects such as mere. Its notice must accompany the binary, and
  its two conditions apply to those portions: no use in systems that can
  purposefully harm people, and no use in creating AI training data. Code
  outside the two crates keeps its own license.
- **Firmware images are not GPLv3.** Those conditions are incompatible with
  GPLv3. Images are MPL-2.0 combined works that include Reticulum-licensed
  portions.
- **RNode firmware (GPL-3.0) is not read.**

The Reticulum protocol itself is public domain, as upstream states in
[Brandolini's Reference](https://reticulum.network/manual/brandolinis.html).
Historical oracle receipts keep their original evidence boundaries: black-box
for the RNS 1.5.2 re-pin; see also the
[September 27 qualification](testing/receipts/rns-1.5.4-repin/README.md).
Comparative reads from September 26 to October 4 are recorded under the
source-review update in
`design_docs/2026-08-25_permissive_radio_protocol_compatibility_survey.md`.
The root [`RETICULUM_LICENSE`](RETICULUM_LICENSE) is the unmodified RNS text.
Oracle-local copies retain their packages' notices.

We acknowledge Mark Qvist and the Reticulum contributors for the protocol
design, documentation, and reference implementations. This project is not
affiliated with or endorsed by the upstream project.
