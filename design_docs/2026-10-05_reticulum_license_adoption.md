# Reticulum License adoption: retinue and outrider

**Date:** 2026-10-05. **Status:** RULED by the owner.

**Supersedes:** for these two crates, the "comparative review only; adaptation
is a separate decision" scope of the
[2026-09-26 source-review policy](2026-08-25_permissive_radio_protocol_compatibility_survey.md#source-review-policy-update-2026-09-26),
and point 2 (firmware images are GPLv3) of the
[2026-07-20 licensing posture](2026-07-20_mesh_household_tulle_tucket_sennet.md#licensing-posture-ruled-2026-07-20-evening-round).

## Decision

1. **`crates/retinue` and `crates/outrider` are under the Reticulum License**
   from this date. Each crate's `LICENSE` carries the upstream text verbatim,
   with this project's copyright line and Mark Qvist's notices for adapted
   portions (RNS for both crates, LXMF for outrider). Releases already
   published to crates.io (retinue and outrider through 0.2.0) were MPL-2.0 and
   keep those terms. The change takes effect with 0.3.0 of each crate. The
   minor bump stops `^0.2` users from picking up the new license through
   `cargo update`.
2. **The reference implementations may be read and adapted** into those two
   crates. That covers RNS and LXMF Python source and the Reticulum manual.
   This is the separate decision the September 26 policy reserved.
3. **Adapted material lands only in retinue and outrider.** The rest of the
   workspace stays MPL-2.0 and receives no RNS- or LXMF-derived code. That
   covers tulle, selvage, sennet, tucket, postilion, seneschal, the radio-*
   crates, the apps, and the firmware crates. If another crate needs adapted
   behavior, it depends on retinue or outrider for it, or the move gets its own
   decision.
4. **RNode firmware source is not read.** It is GPL-3.0. Reading it would put
   tulle's and the firmware's RNode work at risk, and GPL code cannot be
   combined with Reticulum-licensed code anyway. `RNodeInterface.py` is part of
   RNS, under the Reticulum License, and may be reviewed. Rule 3 keeps
   adaptations from it out of tulle.
5. **Firmware images are no longer GPLv3.** Every image links retinue. The
   T114 image does so directly; the V4 default does so through
   `radio-hand/control-retinue`. The Reticulum License's harm and AI-training
   conditions are further restrictions that GPLv3 section 10 forbids. An image
   is a combined work: MPL-2.0 workspace code, Reticulum-licensed
   retinue/outrider portions, and permissive vendored code. MPL-2.0 section 3.2
   still requires a source offer for every distributed image, so the
   source-with-image practice continues. What is lost is GPLv3's
   installation-information clause.

## Amendment, 2026-10-09: tulle and radio-hand

**Status:** RULED by the owner.

- **`crates/tulle` and `crates/radio-hand` join retinue and outrider under the
  Reticulum License.** The interface batch
  ([RNS 1.5.7 interface review](2026-10-09_rns_157_interface_review.md))
  brought RNS's host-side `RNodeInterface.py` semantics into tulle's RNode
  host, and the matching device answers into radio-hand. Rule 3 above kept such
  adaptations out of those crates; the owner chose to relicense them rather
  than move the code. Each carries the Reticulum License and a NOTICE ledger.
  tulle 0.1.0 was published under MPL-2.0 and keeps those terms; the change
  takes effect with tulle 0.2.0. radio-hand was never published.
- **Dependents.** Every crate and app that links tulle or radio-hand already
  linked retinue, so no new combination arises.
- **RNode firmware is still not read (rule 4).** During the batch a reviewer
  agent fetched RNode firmware source and modelled its airtime-lock echo. Before
  merge, the echo values in tulle's test, radio-hand's `rnode_air` example and
  the `interop_rnode_air` gate were replaced with an arbitrary differing value
  justified by RNS alone (`RNodeInterface.py` 667-692, 896-925: echoes are
  recorded, never validated). No firmware text or code entered the repository.

## Why

- **Adaptation requires it.** Upstream's manual (RNS 1.5.5,
  [Brandolini's Reference](https://reticulum.network/manual/brandolinis.html))
  treats a port or translation of RNS as a derivative of licensed work. Such a
  derivative must carry the copyright notice and honor both conditions. An
  MPL-2.0 retinue could not lawfully contain adapted RNS code.
- **Upstream draws the line at the implementation, not the protocol.** The
  same chapter keeps the protocol in the public domain. It also states that
  the AI condition does not forbid machine assistance in development. What it
  forbids is ingesting the work into training.
- **It ends the hedged posture.** Since September 26 this work has been
  described as "source-informed, not strict clean-room." Carrying the upstream
  license replaces case-by-case independence arguments with the upstream
  terms.
- **Historical inputs.** Prns (withdrawn September 26) and the Beechat crate
  shaped early retinue. Upstream's chapter documents Prns as machine-derived
  from RNS. This decision does not certify those inputs. It means retinue now
  carries the terms that would apply if they were RNS-derived.

## Consequences

- **Dependents.** Everything that links retinue or outrider distributes
  Reticulum-licensed code. That includes postilion, seneschal, signalman,
  signalman-desktop, radio-hand with its retinue features, the firmware, and
  mere. Binaries must carry its notice, and its conditions apply to those
  portions. The dependents' own code keeps its own license.
- **Package manifests.** Manifests under `firmware/packages/` describe images
  built at their recorded `source_revision`. Images built before this change
  are correctly MPL-2.0. Images built from a later revision should declare
  `MPL-2.0 AND LicenseRef-Reticulum` and name the Reticulum License in
  `notices`.
- **mere** (MIT/Apache) takes retinue as a git dependency on `main`, behind an
  optional, default-off backend. Builds that enable it ship Reticulum-licensed
  code. mere's own license gate, if any, needs the same exception.
- **crates.io and license gates.** The Reticulum License has no SPDX
  identifier and is not OSI-approved. The manifests use `license-file`, so
  crates.io shows a non-standard license. This repository's `deny.toml`
  clarifies both crates as `LicenseRef-Reticulum` and allows that identifier.
  Downstream gates need the same addition.
- **GPL dependencies**, already a red line, are now ruled out twice over.

## Working rules for review and adaptation

- **Pin what you read.** Record the package, version, and revision. GitHub
  `master` can lag PyPI: on this date RNS 1.5.6 and 1.5.7 were published only
  to PyPI. When reading a PyPI release, record the artifact filename and
  sha256.
- **Record each adaptation in the crate's `NOTICE`.** Give the upstream file
  and line range, the revision, and the target file. The September 26
  provenance practice for comparative reads continues alongside.
- **The owner is the agent of the work.** Adaptations are directed, reviewed,
  and explainable by the owner. Machine assistance is fine; bulk machine
  translation is not the method. Upstream's test: never sign what you cannot
  answer for.
- **Honor the AI-training condition in tooling.** Read upstream source only in
  tools and accounts whose conversations are not used for model training. For
  Claude, that means the model-training setting is off for any account used to
  read RNS or LXMF source.
- **Read upstream, not intermediaries.** Third-party RNS derivatives are not
  inputs. That includes Prns, rns-rs, and the reproduced-source comment blocks
  in microReticulum.
- **History stays as recorded.** Historical receipts keep their evidence
  boundaries. New work is described as source-informed or adapted, never as
  clean-room.

## Upstream state checked on 2026-10-05

| Project | Latest | License text |
| --- | --- | --- |
| RNS | 1.5.7 on PyPI (uploaded 2026-10-05). GitHub `master` is `e40191b`, past tag 1.5.5; 1.5.6 and 1.5.7 are not pushed. | Reticulum License. The text at `master` (LICENSE last changed `b280a73`, 2026-01-10) is byte-identical to the root `RETICULUM_LICENSE`. The 1.5.7 wheel ships no license file; its metadata says "Reticulum License". |
| LXMF | 1.2.0 (PyPI and tag `c3ff2d6`, 2026-09-30) | Same terms, `Copyright (c) 2020-2025 Mark Qvist`. Unchanged since `1bdcf6a` (2025-04-15). |
| RNode_Firmware | 1.86 (2026-04-24) | GPL-3.0 since `b85f660` (2022-11-10). |
