# FS4 key custody process, and the FS5 seizure inventory

**Date:** 2026-08-10. **Lane:** Assurance (ASSURE5).
**Status:** FS5 closed with an enforced check. FS4's process half written; its
physical half belongs to Distribution and has not run.

The security posture states both gates. This document supplies the process
policy FS4 asks for and the enumerated inventory FS5 asks for, and says plainly
which parts are claims about design rather than receipts from hardware.

## FS5: what a seized node yields

The seizure paragraph is a design target, and a target with no check drifts. It
now has one: `validation/security/flash_classification.py`, registered as the
`flash-classification` suite and run on every push.

The audit reads the persisted schema out of
`crates/radio-hand/src/settings.rs`, requires every field to carry a
classification in `validation/security/flash-policy.toml`, refuses a
classification whose field no longer exists, and fails on any persisted field
name matching a forbidden pattern. Today it reports:

| record | classification |
| --- | --- |
| `identity` | pseudonymous |
| `channel` | public |
| `region` | public |
| store record header | public |
| crash residue | public |

That is the whole inventory, and it matches the seizure paragraph. The one
pseudonymous item is the node's own relay keypair, which the paragraph already
prices: an attacker gets one impersonable relay until it is de-listed.
`channel` and `region` are observable by anyone in radio range, because they
determine what the radio does on air. The store header is a magic value, a
version, a length, a sequence, and a CRC. The crash residue is a panic message
from our own open-source binary, and it lives in noinit RAM rather than flash,
so it survives a reset and does not survive power removal: a board that arrives
seized arrives without it.

Three properties of the check are worth stating, because each is a way the
paragraph could have quietly stopped being true.

**A new field fails the build.** Adding a field to `Settings` without
classifying it is an error naming the field and the file to classify it in.
This is the standing rule from the posture made executable.

**A forbidden name fails the build wherever it is persisted.** The pattern list
covers host network credentials, operator secrets, site inventory, contacts,
and coordinates, and it scans the settings schema and every board's flash-backed
store. The V4 is the live risk here: it has Wi-Fi, and the donor project stores
its PSK on flash in cleartext. The pattern lands before the feature does.

**A rejected proposal stays rejected.** A field may be listed as `forbidden`
with a reason. If it then appears in the schema, the audit fails and quotes the
reason. That is how a decision survives the departure of everyone who made it.

The audit self-tests its own failure modes. `--self-test` drives the decision
code through an unclassified addition, a forbidden field that showed up anyway,
a stale classification, a reasonless one, an invented classification, and the
forbidden-name scan including a comment that names a PSK to explain its
absence and must not trip. This lane has already found one check that had never
executed; a check nobody has watched fail is the same mistake wearing a
different hat.

## FS4: key custody

### Three authorities, deliberately separate

**Config authority, warm.** Signs commands to a running node: the FS2 envelope.
Lives with the operator, in signalman, on a host. It is warm because commands
are ordinary and frequent. Its blast radius is bounded by what FS2 permits: the
node holds only the public half, a command carries its own counter, and no
command can rewrite the firmware.

**Firmware authority, offline.** Signs images. Kept offline, on removable
media, not on a build machine and not in a repository. Used at signing time and
put away. Today it signs nothing on-device, because there is nothing on-device
that verifies it: see the per-board reality below.

**Publisher authority, for packages.** Linkboy verifies a package before it
opens a device. That signature proves who published, and the signed Merely
index remains the authority that admits a network-delivered package. This is a
distribution key, not a device key, and it does not become a Linkboy-global
trust root.

The rule that makes the split load-bearing: **an authority that can be used
remotely must not also be able to replace firmware.** Config authority is warm
precisely because it cannot.

### Per-board reality, stated honestly

**Heltec V4 (ESP32-S3).** The part supports secure boot v2 plus flash
encryption with eFuse-burned keys, which is the mechanism that would put a
verification key beyond the application's reach. It is not enabled on any board
we ship, and no image has been built or booted against it. The path is
documented as available; that is a different claim from working, and this
document does not make the second one.

**T114 (nRF52840).** APPROTECT has a real voltage-glitch history, so the design
assumes T114 flash is readable and does not lean on the datasheet. There is no
bootloader verification key on this board worth trusting, and adding one would
be theatre. What protects a T114 instead is having nothing worth stealing on
it, which is rule 1 and is enforced by the FS5 audit above.

**Both boards, today.** Firmware authority is physical: linkboy runs at the
owner's bench, over USB, with the device in the owner's hand. FCC v1 is stock
hardware plus user-flash, so there is no remote firmware authority to
compromise. This is the fleet-key liability being absent by posture rather than
mitigated, and it is worth defending as a property rather than treating as a
stage to grow out of.

### The OTA gate

> **No over-the-air firmware update capability ships before the key split,
> offline custody, and bootloader verification are in place.**

There is no OTA surface in the tree today. The on-device UI plan reserves an
OTA screen and explicitly defers it, and the flashing plan states that it is
not an OTA plan disguised as an installer. The gate is therefore currently
satisfied by absence, which is the cheapest way to satisfy it.

The gate is not self-enforcing, so it goes in the release checklist below,
which is where a release actually gets made. A single remote update authority
over N sold radios, compromised, is a recall plus an FCC problem. That is why
this is a precondition for the lane existing and not one hardening item among
many.

### Release checklist

Run for every firmware release. No item may be waived silently: a waiver is
recorded in the release receipt with who granted it and why. Prns shipped 0.3.4
under a recorded maintainer override waiving physical acceptance, and recording
the waiver is the part of that worth copying.

1. **OTA gate.** Confirm the release adds no remote firmware update path. If it
   does, stop: the key split, offline custody, and bootloader verification are
   preconditions, and none of them is in place.
2. **Key tiers.** Confirm no config-authority key can replace firmware, and
   that no firmware-authority private key is present on a build machine, in the
   repository, or in CI outside a protected environment.
3. **Reproducible build.** Two independent builds of the same source revision,
   byte-compared before anything is signed.
4. **Sign without rebuilding.** Sign the compared artifact. A signing step that
   rebuilds signs something nobody compared.
5. **Provenance recorded.** License, source revision, corresponding-source
   location, build material, and the offer facts. The artifact-identity shape
   (route, checksum route, byte length, SHA-256) is useful and is not by itself
   a GPLv3 corresponding-source offer.
6. **Pinned actions.** Third-party CI actions pinned by SHA, not by tag.
7. **Attestation is supplemental.** Sigstore or equivalent may be added; it
   never replaces the pinned key.
8. **Rollback retained.** The complete previous candidate bundle is kept, so a
   rollback is a restore rather than a rebuild.
9. **Audits green.** `validation/run.py verify`, the unsafe audit, and the
   flash-classification audit all pass at the exact release commit.
10. **Seizure paragraph re-read.** If the release adds a persisted field, a new
    radio, or a network credential, re-read the paragraph and confirm it is
    still true. The audit catches the field; only a person catches the meaning.

## What this does not close

- **Every physical FS4 receipt.** A modified application image failing to boot
  on a V4, and rewriting a stored verification key from application code being
  demonstrably impossible, are hardware claims. They need a bench, a board, and
  a burned eFuse, and the Distribution lane owns them. Nothing here should be
  read as evidence that secure boot works on our hardware, because it has not
  been tried.
- **The T114 bootloader question**, which is closed by design rather than by
  mechanism: the board is assumed readable and given nothing to lose.
- **Allowlist lifecycle**, carried over from
  [the FS2 decision](2026-08-10_fs2_command_carrier_decision.md): de-listing a
  seized relay and rotating operator keys are themselves signed commands, so
  who signs the command that revokes the only key is still open.
- **FS3**, the durable counter, which is the next gate and now has a settled
  command grammar to bind.
