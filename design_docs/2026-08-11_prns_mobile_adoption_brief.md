# Prns and the Retinue mobile lane

**Date:** 2026-08-11
**Status:** researched recommendation; no dependency decision or code import has
yet been made
**Question:** should the Retinue mobile app learn from Prns, reuse Prns, or
adapt Prns code?
**Recommendation:** collaborate dependency-first. Treat Prns as the preferred
Reticulum and Bluetooth provider wherever its public seams fit; ask upstream
for a narrower reusable seam where they do not. Use source as a donor only
when a dependency boundary is genuinely impractical, and use it only as an
oracle when independent implementation is itself the requirement.

## Executive decision

There is no good reason to build a second iOS Reticulum host and a second
CoreBluetooth data plane merely to preserve the appearance of independence.
Prns is a ground-up Rust implementation under `MIT OR Apache-2.0`, welcomes
contributions, publishes `personal-rns`, and already runs its Rust engine and
Bluetooth Auto interface inside an iOS application. We built that application
for an iOS device and simulator on the new Apple Silicon iMac, installed it on
an iPhone, and launched it successfully. That is not evidence that every
production lifecycle case is closed, but it is much stronger evidence than a
code-reading exercise.

Retinue should not disappear into Prns, either. The two workspaces currently
own different things:

- Prns owns a complete Reticulum engine, its host runtimes, and a substantial
  cross-platform interface family.
- Retinue owns another Reticulum implementation plus the protocol-neutral
  radio boundary, the Sennet and Tucket personalities, on-board personality
  residency, and the proposed neighbor-signaling policy that decides which
  radio citizenship is useful now.

Making `personal-rns` the sole Retinue core would put two existing Reticulum
engines behind one product name and leave identity, routing, persistence, and
event ownership ambiguous. Reimplementing all of Prns would duplicate the
most platform-sensitive work. The useful composition is therefore:

```text
Retinue mobile application
  owns: people, messages, capability/signaling policy, radio citizenship UX
    |
    +-- Retinue provider
    |     Retinue + Outrider behavior owned in this workspace
    |
    +-- Prns provider
    |     Personal RNS behavior and Reticulum Bluetooth Auto
    |     consumed from Prns, with improvements contributed upstream
    |
    +-- Sennet provider       Meshtastic-compatible personality
    +-- Tucket provider       MeshCore-compatible personality
    |
    +-- local carriers
          Bluetooth, USB, Wi-Fi/LAN; a carrier is not the policy
```

This is a provider boundary, not a claim that every provider must immediately
share one Rust trait. The first spike should discover the narrowest honest API
by carrying one message over one real Bluetooth path.

## What the Personal Hopspot experiment established

The experiment had two purposes, and achieved both.

### The Apple toolchain and device path work

On this machine we proved:

1. the Rust `aarch64-apple-ios` static library builds;
2. the full Xcode application builds for an Apple Silicon simulator;
3. Xcode signing can install a development build on the attached iPhone; and
4. Personal Hopspot launches on that phone.

The signing exercise is reusable knowledge. It was not the product decision.

### Prns is a viable collaborator, not just an example screenshot

At the researched Prns revision
[`58a87854f4143901d09d6da71f1033e8cf81240c`](https://github.com/KenAKAFrosty/Prns/tree/58a87854f4143901d09d6da71f1033e8cf81240c):

- `personal-rns` is version `0.3.5`, is published, and splits its runtime and
  interface families through Cargo features such as `tokio-host`,
  `embassy-host`, and `bluetooth-auto`.
- `personal-hopspot-ios` is a Rust `staticlib` behind a small C ABI. Its Swift
  shell supplies application lifecycle and presentation while Rust owns the
  engine, persistence, interfaces, and shared renderer.
- its iOS dependency graph selects `personal-rns` with `tokio-host`, TCP,
  Wi-Fi Auto, Bluetooth Auto, and persistence features, and selects
  `prns-ffi` on iOS for the native CoreBluetooth backend.
- the Apple BLE backend acts as central and peripheral, uses restoration-aware
  CoreBluetooth managers, carries a GATT floor, can negotiate an L2CAP data
  plane, and recognizes the Columba GATT profile as a compatibility peer.
- the same Bluetooth Auto concepts have bounded Embassy implementations for
  embedded targets as well as Tokio host implementations.

The important architectural fact is that Prns already separated platform BLE
from protocol supervision. `prns-core` declares public `BleBackend`,
`BleLink`, `BleSource`, and `BleSink` seams. `prns-ffi` implements the unsafe
Apple calls outside the pure engine, and the Tokio interface crate assembles
that backend into `AutoBle`. This is precisely the seam we would otherwise
spend time rediscovering.

There are still readiness limits. Prns describes the iOS face as pre-1.0 and
shipping, with a simulator gate and device smoke, while separately saying that
formal production hardware qualification and continuous background execution
are not complete. Apple's own CoreBluetooth documentation also makes clear
that background central/peripheral operation is constrained: apps must declare
the respective background modes, discovery is throttled, peripheral
advertising changes, and state preservation/restoration is opt-in. A successful
foreground install is therefore the beginning of Retinue's iOS evidence, not
the end.

## The adoption ladder

Use the highest rung that preserves correct ownership. Do not begin at “copy.”

### 1. Depend on Prns

Choose a normal Cargo dependency when the code and its invariants belong in
Prns and the public API is sufficient. This is the default for:

- Reticulum supplied as a Prns provider;
- Bluetooth Auto protocol supervision;
- the Apple CoreBluetooth backend;
- the Embassy Bluetooth Auto implementation on supported boards; and
- reusable wire types or test fixtures that Prns intentionally publishes.

Benefits:

- one owner fixes platform lifecycle and BLE behavior for every consumer;
- Retinue receives upstream testing and security work;
- fixes made for Retinue can be submitted as coherent Prns pull requests; and
- provenance remains ordinary dependency metadata instead of archaeology.

Cost:

- current public types may carry Reticulum-specific assumptions that are too
  high-level for a protocol-neutral carrier;
- bringing both Retinue and Prns routing engines into one process can increase
  binary size and create two persistence/identity authorities; and
- a git dependency may be needed before every necessary mobile crate has a
  registry-ready package.

Those are reasons to measure and improve a boundary, not reasons to fork it.

### 2. Contribute a narrower seam upstream

If Retinue needs the Apple or Embassy carrier but `AutoBle` is coupled too
tightly to the Prns manifold, first propose an upstream extraction or public
adapter. Candidate outcomes include:

- exposing the existing frame source/sink and peer-event layer as a supported
  consumer surface;
- making backend construction/lifecycle available without requiring a Prns
  routing node; or
- accepting a consumer-owned frame supervisor while Prns retains the GATT,
  L2CAP, restoration, and flow-control implementation.

This is the most respectful answer to “we need the same machinery but not the
same owner.” It leaves the difficult implementation where it already has
maintainers and gives Prns a more reusable API. The request should begin with a
small issue or design discussion and a working branch, not with a speculative
large refactor.

### 3. Adapt Prns code as a donor

Use source as a donor when the desired component cannot sensibly remain a
dependency—for example, a small Swift host shell that must live inside a
Retinue Xcode project, or a generic extraction that upstream declines because
it does not fit Prns's product boundary.

Donor rules:

1. identify the exact Prns file and commit before editing;
2. preserve copyright and license notices;
3. keep copied/adapted work in clearly named files where practical;
4. add a repository notice recording the source URL, commit, files, license,
   and the nature of Retinue's modifications;
5. retain the applicable MIT and Apache-2.0 texts and satisfy Apache NOTICE
   obligations when applicable;
6. say “adapted from Prns,” not “independently implemented”; and
7. offer generally useful fixes upstream even if the final integration remains
   local.

Retinue already has a precedent in `crates/tucket/NOTICE`: permitted source
adaptation is made visible instead of being hidden behind a vague claim of
compatibility. A Prns adaptation should meet at least that standard. Because
Retinue's default license is MPL-2.0 and Prns offers MIT or Apache-2.0, the
repository can carry the works together, but file-level licensing and notices
must remain explicit. This brief is an engineering policy, not legal advice;
release review should verify the final file and binary graph.

### 4. Use Prns as an oracle

Choose oracle-only treatment when independence has concrete value:

- Retinue is maintaining its own Reticulum implementation and needs
  cross-implementation conformance evidence;
- a security or interoperability claim requires two implementations that do
  not share the code under test;
- the desired architecture has materially different invariants; or
- importing an implementation would erase provenance promised by an existing
  Retinue protocol crate.

Oracle does not mean pretending the source is invisible. It means deriving
tests from public behavior, protocol documents, captured packets, and
cross-implementation experiments rather than porting the implementation. Record
which observations came from Prns and which correctness rationale is
independent.

### 5. Implement independently only after the preceding rungs fail

Independent implementation is justified when it is genuinely different,
better bounded for Retinue's needs, or required for trustworthy conformance.
“We already have a repository” is not sufficient justification for repeating a
working iOS lifecycle or BLE state machine.

## Component-by-component recommendation

| Component | Recommended relationship | Why |
|---|---|---|
| Reticulum routing and links inside a Prns personality | direct `personal-rns` dependency | This is Prns's core product and already has host/embedded runtime splits. |
| Retinue's existing Reticulum core | oracle/interoperation peer | Retinue already owns this implementation; replacing it is a separate decision, not mobile scaffolding. |
| Apple CoreBluetooth backend | dependency, or upstream a narrower public seam | It contains platform FFI, central/peripheral roles, restoration, flow control, GATT, and L2CAP work that should have one owner. |
| Embedded Bluetooth Auto | dependency plus upstream contributions | The Embassy implementation is directly aligned with the nRF52840 lane and bounded-resource concerns. |
| Personal Hopspot Rust lifecycle | study, then dependency or donor by boundary | Its restartable engine, persistence path, diagnostics, and C ABI are valuable; Retinue must not inherit Hopspot-specific node ownership accidentally. |
| Swift/Xcode shell | donor/template with attribution | Xcode project structure and a small FFI host are integration scaffolding, not Retinue's product semantics. |
| Personal Hopspot visual design and product identity | do not copy by default | Retinue's application has a different job: people, messages, neighboring capability, and radio citizenship. |
| Bluetooth behavior tests | shared fixtures plus black-box cross-tests | Both projects benefit from reproducible peer evidence; independent end-to-end tests catch shared assumptions. |
| Meshtastic and MeshCore personalities | Retinue-owned providers | Sennet and Tucket already carry their own provenance and protocol responsibilities. |
| Neighbor signaling and personality selection | Retinue-owned | This is the differentiating policy that invites several standards into one retinue; it is not Reticulum transport machinery. |

## The mobile boundary Retinue should own

The app is not “Personal Hopspot with a different logo.” Its minimum durable
model is:

```text
person/contact
  has one or more reachable identities/endpoints

message intent
  may be delivered by any compatible local provider

neighbor capability
  says which protocol personalities and carriers are present or requested

personality policy
  chooses whether a board stays home, visits another mesh, or declines

receipt
  says what provider/carrier actually accepted, transmitted, delivered,
  deferred, or refused the message
```

The carrier must not silently choose the personality. Bluetooth can carry a
Prns peer session, an RNode/board control session, a Meshtastic client API, a
MeshCore client API, or Retinue's own signaling. “Bluetooth connected” is
therefore not a complete routing fact. The app needs a typed capability and
session event above CoreBluetooth and below the provider policy.

On the board, `radio-hand::HostLink` already states the complementary shape: a
personality-agnostic byte pipe with session lifecycle, with BLE fragmentation
hidden inside the transport. `Channel` owns the selected on-board personality.
The live `CMD_CONFIG` path can retune PHY parameters without a reboot; full
teardown-correct hot switching among resident on-board channels remains a
separate murmuration milestone. The mobile design should preserve that
distinction in its UI and claims.

## First vertical slice

Build the smallest path that answers the adoption question before designing a
full app:

1. Create a minimal Retinue iOS shell with a Rust static library and one screen:
   nearby sessions, one message composer, and an event/receipt log.
2. Integrate `personal-rns` as a Prns provider without copying its engine.
3. Prove iPhone-to-laptop text over Prns Bluetooth Auto in the foreground.
4. Feed the same user-level message intent through one Retinue-owned provider
   on the laptop, so the app boundary proves it is not synonymous with Prns.
5. Connect one board over BLE as a local radio session and expose its current
   personality and PHY separately.
6. Send a capability/request signal that asks the board to use a named
   personality. For the first demonstration, a host-driven live PHY retune is
   sufficient if the UI labels it honestly; resident hot-channel switching is
   not yet claimed.
7. Preserve an event receipt for every transition so a meeting demo can show
   what happened even if RF conditions are poor.

The first slice intentionally has no address book synchronization, background
guarantee, multi-radio scheduler, or automatic bridge between encrypted
protocol domains. It proves the ownership boundary and one useful message.

## Spikes and acceptance gates

### P0: Prns provider boundary

- Build a scratch static library that imports `personal-rns = 0.3.5` (or the
  pinned Prns revision if an unpublished crate is required).
- Start and stop it repeatedly through the C ABI without leaking a runtime or
  duplicating a singleton.
- Surface typed interface, peer, message, and failure events to Swift.
- Record binary size, clean-build time, resident memory, and cold start.
- Decide which identity and persistence roots belong to Prns and which belong
  to the Retinue application.

**Pass:** the Prns engine is a contained provider, not a second application
hidden inside the process.

### P1: Bluetooth seam

- Prove iPhone-to-Mac and iPhone-to-Asus foreground messages over Bluetooth
  Auto where platform support permits.
- Determine whether the existing public `AutoBle` surface is sufficient.
- If Retinue needs raw peer frames outside a Prns node, draft the smallest
  upstream API proposal and validate it in a branch.
- Exercise Columba compatibility separately from native Prns Bluetooth Auto;
  do not treat one receipt as proof of both.

**Pass:** no Retinue-local CoreBluetooth state machine exists unless the
upstream route was tried and its failure is recorded.

### P2: iOS lifecycle

- Verify permission-denied, Bluetooth-off, suspend/resume, termination and
  restoration, radio loss, and repeated engine restart.
- Test central and peripheral roles independently.
- Verify the exact background behavior on the target iPhone/iOS version;
  background declarations are capabilities with operating-system limits, not
  a promise of continuous execution.
- Keep persistence valid after forced termination and application upgrade.

**Pass:** the UI distinguishes unavailable, searching, connected, suspended,
restored, and failed states without claiming a message was delivered merely
because CoreBluetooth connected.

### P3: board carrier

- Add one BLE `HostLink` implementation, beginning with the nRF52840 T114.
- Keep ATT fragmentation, MTU, and connect/disconnect behavior in the carrier.
- Keep command parsing and personality ownership in `radio-hand`/`Channel`.
- Measure queue bounds and memory pressure under concurrent host and RF input.

**Pass:** USB and BLE drive the same command semantics and emit equivalent
receipts.

### P4: collaborative upstream loop

- Open one focused Prns discussion or issue with the measured consumer need.
- Submit generic improvements to Prns with its validation ladder and
  contribution conventions.
- Keep Retinue adapters small and version-pinned until the upstream surface is
  released.
- Add a cross-project compatibility test that can be reproduced without either
  maintainer's private environment.

**Pass:** the relationship produces an upstream improvement or a documented
boundary decision, not an unacknowledged private fork.

## Decision rules for future work

When someone is about to implement something already present in Prns, answer
these in order:

1. **Does Prns own the concept?** Depend on it.
2. **Is the implementation useful but the public seam too broad?** Propose and
   contribute a narrower seam upstream.
3. **Must the code physically live in Retinue?** Adapt it with commit-pinned
   provenance and notices.
4. **Does independence strengthen a conformance or security claim?** Use Prns
   as an oracle and record the independent rationale.
5. **Are the invariants actually different?** Implement locally and document
   the difference.

If none applies, duplication is probably the wrong default.

## Tomorrow's demo posture

The app installation is a successful toolchain and architecture reconnaissance
receipt. It is not yet the Retinue multi-protocol mobile app. For the meeting,
the honest story is:

- Personal Hopspot on the phone proves a Rust Reticulum node with Wi-Fi and
  Bluetooth can live behind a native iOS face.
- The personality filmstrip proves the UI vocabulary and shows the state
  transitions Retinue intends to govern; it does not claim that all on-board
  hot switching is implemented.
- The three existing phone apps—RetiChat, Meshtastic, and MeshCore—remain the
  available protocol-specific faces for live radio evidence.
- Retinue's differentiator is the signaling and policy layer that can discover
  those capabilities, request a radio personality, and present one message
  intent across them.

That is a coherent demonstration of direction with explicit borders around
what is already built.

## Research basis

Primary sources inspected at Prns commit
[`58a87854`](https://github.com/KenAKAFrosty/Prns/tree/58a87854f4143901d09d6da71f1033e8cf81240c):

- [Prns README and license statement](https://github.com/KenAKAFrosty/Prns/blob/58a87854f4143901d09d6da71f1033e8cf81240c/README.md)
- [Prns contribution guide](https://github.com/KenAKAFrosty/Prns/blob/58a87854f4143901d09d6da71f1033e8cf81240c/CONTRIBUTING.md)
- [`personal-rns` feature manifest](https://github.com/KenAKAFrosty/Prns/blob/58a87854f4143901d09d6da71f1033e8cf81240c/personal-rns/Cargo.toml)
- [iOS host README](https://github.com/KenAKAFrosty/Prns/blob/58a87854f4143901d09d6da71f1033e8cf81240c/personal-hopspot/mobile/ios/README.md)
- [Bluetooth backend traits](https://github.com/KenAKAFrosty/Prns/blob/58a87854f4143901d09d6da71f1033e8cf81240c/prns-core/src/interfaces/bluetooth_auto/backend.rs)
- [Apple CoreBluetooth backend](https://github.com/KenAKAFrosty/Prns/tree/58a87854f4143901d09d6da71f1033e8cf81240c/prns-ffi/src/bluetooth_auto/macos)

Platform constraint sources:

- [Apple Core Bluetooth overview](https://developer.apple.com/documentation/corebluetooth)
- [Apple background execution modes](https://developer.apple.com/documentation/xcode/configuring-background-execution-modes)
- [Apple state preservation and restoration guide](https://developer.apple.com/library/archive/documentation/NetworkingInternetWeb/Conceptual/CoreBluetooth_concepts/CoreBluetoothBackgroundProcessingForIOSApps/PerformingTasksWhileYourAppIsInTheBackground.html)

Relevant Retinue authorities:

- [`2026-08-09_channel_murmuration.md`](2026-08-09_channel_murmuration.md)
- [`2026-08-06_signalman_founding.md`](2026-08-06_signalman_founding.md)
- [`2026-07-31_retinue_small_plan.md`](2026-07-31_retinue_small_plan.md)
- [`2026-07-19_modem_embedded_and_meshtastic_research.md`](2026-07-19_modem_embedded_and_meshtastic_research.md)
