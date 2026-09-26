# Third-party notices

Retinue is licensed under the Mozilla Public License, Version 2.0 (see
`LICENSE`), including the firmware. This file aggregates the third-party work
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
  current captures use project-selected inputs and pinned RNS 1.5.2.
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

The Reticulum protocol specification and manual are public domain. The Python
reference implementation was used as a black-box oracle for the recorded
RNS 1.5.2 re-pin qualification. That receipt describes its precise evidence
boundary. On September 26, the owner reported the first limited review of RNS
implementation source and clarified that no RNS implementation code has been
copied or translated into Retinue. The license is included to identify and
acknowledge the reference implementation, not to declare an RNS-derived port.
Comparative review is the current scope; adaptation is a separate decision.
See the source-review update in
`design_docs/2026-08-25_permissive_radio_protocol_compatibility_survey.md`.
We acknowledge Mark Qvist and the Reticulum contributors for their protocol
design, documentation and reference implementation. The full upstream RNS
license is included at [`RETICULUM_LICENSE`](RETICULUM_LICENSE) in good faith
and for reference. Including it does not apply it to all Retinue code or offer
an alternative MPL grant for upstream code. Oracle-local copies retain the
notices for their reference packages. This project is not affiliated with or
endorsed by the upstream project.

### Scope of the included Reticulum License

The root license copy identifies and acknowledges the reference implementation.
We respect its terms in our own use of that implementation. This statement is
not an additional condition on recipients of independently authored Retinue
code: that code remains MPL-2.0, with no added harm or AI-use restrictions.
The protocol's public-domain status and the implementation's separate terms are
explained in [Brandolini's Reference](https://reticulum.network/manual/brandolinis.html).
Copying or translating protected implementation material, including through an
intermediary, would require its applicable terms; this notice is not a waiver
or certification that every historical input has been independently cleared.
