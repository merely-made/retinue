# Catalog authentication and activation foundation

Date: 2026-08-20

This closes the host-side foundation that must precede Bluetooth, Wi-Fi, or
LoRa firmware delivery. It does not claim an OTA bearer or an on-device update
implementation.

## Authority

Linkboy now distinguishes three things that were previously easy to collapse:

1. `PackageIndex` is parsed catalog data.
2. `CatalogTrust` is owner-selected local policy containing publisher names,
   key ids, and Minisign public keys.
3. `AuthenticatedPackageIndex` exists only after the index's detached
   signature verifies over domain-separated canonical bytes against that
   policy.

`catalog-auth INDEX TRUST` exercises the strict path. Network fetch and staging
code can take `AuthenticatedPackageIndex`, so an unsigned parsed index cannot
cross that boundary by type accident. The ordinary `catalog` command remains
a structural validator for checked-in and staging fixtures; it makes no
authentication claim.

The public package index is still unsigned. No production Merely Made trust
key or signature was invented for this change. Network catalog use therefore
remains blocked on the offline-key ceremony, checked-in public trust root, and
a real signature over the canonical bytes.

Each catalog package now carries a positive monotonic `release_sequence`.
Human version strings remain presentation; staging uses the sequence to refuse
rollback.

## Staging, activation, and rollback

`apps/linkboy/src/update.rs` provides a persistable state model beneath every
future bearer:

- a stageable release is derived only through an authenticated catalog and a
  package whose payload digests have all verified;
- the release identity includes the ordered payload-set digest, expected
  application, and monotonic release sequence;
- staging refuses an older or equal sequence and a different package id;
- a rollback-capable target selects a staged image for a trial boot, confirms
  only the expected returned application, and retains the last confirmed image
  until then;
- an unconfirmed trial can return to the last confirmed image;
- an `ExternalRecoveryOnly` target may stage bytes but cannot claim safe
  activation.

The activation modes are capability statements, not board-name guesses. No
current V4 image is marked rollback-capable by this slice; its partition and
bootloader receipt is still owed. The current T114 stock bootloader route is
modeled as external recovery only, so staging v52 does not become a false OTA
claim.

## Verification

Commands run from the clean Retinue main checkout with isolated Cargo targets:

```text
cargo test -p linkboy --locked --offline
76 passed; 0 failed

linkboy catalog firmware/packages/index.toml
package index retinue.package-index/v1 publisher=Merely Made version=3 packages=4
```

The tests cover Minisign acceptance and changed-byte rejection, signature
omission from canonical bytes, missing/untrusted signature refusal, the
retained public catalog, monotonic staging, exact application confirmation,
rollback, cross-package refusal, and the T114 recovery-only boundary.

## Remaining done conditions before a bearer

- create the offline Merely Made catalog key and publish only its public half;
- sign package-index version 3 and retain a successful `catalog-auth` receipt;
- choose and receipt an actual inactive-slot/rollback mechanism for each board
  allowed to activate remotely;
- persist `UpdateJournal` atomically on that target;
- define chunk authentication, resume, rate, and power-loss rules for the
  bearer without weakening the authority above.
