# Pocket NFC Probe

Disposable native iPhone probe for the
[magnetic pocket-node plan](../../design_docs/2026-09-07_magnetic_pocket_node_plan.md),
PN2 platform preparation. This is not the Signalman mobile host or a dock
protocol implementation. It has no Rust, Mere, or firmware dependency.

## Scope

- Explicit foreground NDEF read and ISO15693 tag detection, using Core NFC.
- One reader session at a time; request invalidation after 30 seconds or when
  backgrounded; wait for its callback before permitting the next scan.
- Record reader availability, activation, detection, cancellation/errors,
  elapsed time, placement, and operator-entered case/orientation notes.
- Atomically retain the latest 128 events in the app's Documents directory.
  The Share button exports JSON only when the user chooses a destination.
- Retain neither tag UIDs nor NDEF content. There is no network client, tag
  write, mailbox command, pairing, credential, or board mutation.

NDEF foreground reading does not prove background NDEF launch. That later leg
needs a supported universal link and domain association. ISO15693 detection
does not prove an ST25DV mailbox, authentication, useful throughput, attached
geometry, charging coexistence, or autonomous Retinue operation. `app_open`
and reader availability do not prove that a physical tag was read.

## Build and install on the M4

Requires Xcode with an iOS SDK, a development team with NFC capability, and a
paired iPhone with Developer Mode enabled. Set the team and device identifiers
locally; do not commit them. The project has no package downloads.

```sh
cd apps/pocket-nfc-probe
xcodebuild -project PocketNFCProbe.xcodeproj -scheme PocketNFCProbe \
  -configuration Debug -destination 'generic/platform=iOS' \
  -derivedDataPath build CODE_SIGNING_ALLOWED=NO build

# Physical install: use the owner's configured Xcode account and team.
xcodebuild -project PocketNFCProbe.xcodeproj -scheme PocketNFCProbe \
  -configuration Debug -destination "id=$PROBE_DEVICE_UDID" \
  -derivedDataPath build DEVELOPMENT_TEAM="$PROBE_TEAM" \
  -allowProvisioningUpdates -allowProvisioningDeviceRegistration build
xcrun devicectl device install app --device "$PROBE_DEVICE_ID" \
  build/Build/Products/Debug-iphoneos/PocketNFCProbe.app
xcrun devicectl device process launch --device "$PROBE_DEVICE_ID" \
  made.merely.retinue.pocket-nfc-probe
```

Provisioning can register this probe's app id and paired device in the owner's
development team. It does not distribute the app. The unsigned build only
checks compilation; install, launch, and physical scans are separate evidence.

## First physical block

1. Open the app, record case/orientation, and select `Loose tag` or
   `Magnetic attachment position` to describe the actual placement.
2. Select `Read NDEF tag` for an ordinary formatted tag, or `Detect ISO15693 tag`
   for an ISO15693 board. Present just one tag and keep the chosen position.
3. Repeat with removal, cancellation, and a scan without a tag. Confirm the
   error/timeout event remains visible. Reopen the app to check local retention.
4. Share the receipt when ready, or collect this app's Documents file through
   `devicectl`. Do not read unrelated phone application containers.

The scanner supports standard NDEF tags; a bank card or transit credential is
not a substitute for the intended dynamic-tag board. Stop at platform evidence
until a protected ST25DV board, MCU peer, and PN1 transcript are available.

## References

- [Apple reader availability](https://developer.apple.com/documentation/corenfc/nfcreadersession-swift.class/readingavailable)
- [Apple ISO15693 tag API](https://developer.apple.com/documentation/corenfc/nfciso15693tag)
- [Apple background tag reading](https://developer.apple.com/documentation/corenfc/adding-support-for-background-tag-reading)
