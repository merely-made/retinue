# Retinue: radio protocols, devices and Signalman

Retinue is the mesh layer of the Merely stack: the Reticulum and LXMF crates,
the radio interface layer, board firmware and the operator apps. The canonical
repository is [merely-made/retinue](https://github.com/merely-made/retinue).
All crate names and source histories are preserved.

The native peer carriers and conversation service that lived in `native/`
moved to [merely-made/murm](https://github.com/merely-made/murm) on
2026-11-25, with `retained-repair` and `transport-conformance`. Murm takes
`retinue` and `truchman` from this repository, and Signalman's optional
`comms` feature takes `mere-comms` from Murm. The
[repository layers plan](design_docs/2026-11-25_repository_layers_plan.md)
records the split. The
[family move plan](design_docs/2026-11-02_murm_family_move_plan.md) keeps the
earlier receiving-build and consumer-cutover status. Development uses Rust
1.99.1 with a shared MSRV of 1.97.1.

The HTTP and WebSocket wire engine (`murm-wire`) and the WebRTC protocol leaf
(`murm-rtc`) that this repository once held in `wire/` moved to Genet on
2026-11-08 as `genet-wire` and `genet-rtc` (`components/genet-wire`,
`components/genet-rtc`, history kept; Mere cycle-breaks D20). Genet was their
only consumer, and they are web-platform protocols rather than mesh carriage.
Netfetcher's browser policy and public crate identity are in Genet beside them,
while hosts select trust, pooling and limits. Stickleback's history, admission and group encryption
live in Eidetic; `murm-sync` owns its live network sessions and address book.
Cross-target and live
network qualification retain their own lanes; see MOVE5 in the family plan.

[Station Custody](crates/station-custody/README.md) is Murm's standalone
Djinn-backed station commissioning adapter. Signalman's desktop uses it by
path; Dramatis/Personae and Djinn retain credential and grant authority.
The adapter stays outside the default radio workspace.

Signalman's desktop Messages face now routes its versioned text-over-LXMF
profile through the shared Comms adapter and the same durable application
journal. With `SIGNALMAN_STATION_PORT` and the existing Djinn custody service,
its attended station actor carries queued intents and persists authenticated
receive events. An unambiguous public identity from saved history supports
offline drafts; transmission still requires the station's current grant.
The face reports recorded queue, handoff, receive and failure facts. This is
software-qualified integration; wider LXMF bodies, other protocol adapters,
real-radio and headed acceptance retain their own gates.

## Radio family


Embedded Rust implementations of three mesh protocols, a shared
radio interface layer, operator apps, and firmware that runs on my limited 
subset of hardware. 

So: what's the idea? Why all three? I wanna make the three more like channels that 
are triggered by a given announce, allowing a node to configure a persona/address 
for each protocol and route traffic correctly. It just seemed like much of the LoRa
radio hardware had the memory space to do that. And the name: a retinue of protocols, 
working together, letting people use each for what they're each good at. There aren't
that many good names that begin with reti-...

On the host side, this is possible with existing projects, but on the embedded side, 
not with Rust, my preferred language ever since I first heard of Servo in 2018.
The path of many such attempts, of late, for good reason. So to be clear, this is not 
a reference implementation of Reticulum, Meshtastic, or Meshcore: it's an experimental implementation.

I use LLMs because I have come to truly enjoy leveraging my English degree in increasingly 
esoteric and elaborate ways. I develop to standards so I can verify functionality,
but the engineering of these projects must and will definitely be improved with time 
before I would release it. If you need reliable protocol implementations, consider the reference implementations.

Each crate has its own README with more detail, and here's a generated overview.

## Crates

| Crate | What it is | crates.io |
| --- | --- | --- |
| `retinue` | Reticulum protocol: identity, announces, links, requests, Resources, routing. `no_std + alloc` core with a tokio shell. | published |
| `outrider` | LXMF: message codec, direct and opportunistic delivery, propagation client and server. | published |
| `postilion` | Shared radio-host library: a Station wraps one identity, one board, an announce cadence and a peer table. | published |
| `seneschal` | Board-management contract: signed control requests and replies, owner and configuration journal. | published |
| `tulle` | Radio interface layer: serial modem control, direct PHY, medium access. | published |
| `selvage` | LoRa PHY profiles shared by host and firmware. | published |
| `radio-face`, `radio-hand`, `radio-mirror` | On-device UI, board runtime, and the UI's browser (wasm) realization. | workspace only |
| `retinue-sim` | In-process multi-node route-trace harness. | workspace only |

Apps: `linkboy` (inspects, plans and flashes firmware packages), `signalman`
(runs a household radio from a serial port), and `signalman-desktop` (its
desktop face, rooted in its own workspace). Firmware for Heltec V4 and Heltec
T114 boards is in `firmware/`; `firmware/packages/` holds package templates and the
index, and a staging step builds the flashable packages (see its README).

## Status (2026-10-06)

- **Releases.** On 2026-10-06, retinue and outrider 0.3.0 were published, the
  first releases under the Reticulum License (see [License](#license)).
  postilion 0.3.0 and seneschal 0.2.0 were published with them; they stay
  MPL-2.0 and move to retinue 0.3. Earlier releases are MPL-2.0.
- **Reticulum.** The live oracle is pinned to RNS 1.6.0 and LXMF 1.3.0.
  The wheels came over Reticulum and were checked against the publisher's
  signing identity. The Windows loopback qualification passed 35 Retinue
  and 18 Outrider gates. See the [wire compatibility plan](design_docs/2026-11-02_wire_compatibility_plan.md)
  for the receipt. The [protocol refresh](design_docs/2026-11-17_reticulum_protocol_refresh.md)
  adds configurable network stamps and fractional LXMF costs, qualified against
  stock over loopback. The 0.2.0
  [link lifecycle](design_docs/archive_docs/2026-11-02/2026-10-02_node_link_lifecycle_plan.md) work
  landed: route learning, `open_link(now)`, link-request expiry, and
  own-echo and duplicate filtering. These results cover the measured
  local-TCP scopes; they are not full upstream parity.
- **Sennet and Tucket.** Since 2026-11-14 MeshCore and Meshtastic meshes are
  reached through a stock companion radio that holds the keys (Mark: "Stock
  radios only"); the frame types Signalman's companion client uses live in
  `apps/signalman/src/radio_comms/companion`, and the two crates left the
  workspace on 2026-11-14 (their names stay published at 0.0.2). The September 30 native exchanges in the [historical receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/sennet-tucket-current/README.md)
  describe code that has since been removed.
- **Firmware.** T114 bounded transport has an on-air receipt from August 13.
  The [firmware refresh](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/firmware-refresh/README.md) and its
  [follow-up](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/firmware-refresh/follow-up/README.md) installed
  clean-pin V4 and T114 images with preserved settings. They passed selected
  RF traffic, a 1024-byte exchange and signed announces across soft resets.
  Resident cancellation is not yet reliable: one isolated run passed and one
  timed out. Full resident, fault and unattended-operation acceptance remain
  open.
- **Signalman desktop** collects observations from identified V4 and T114
  boards, with keyboard export and reload verified. Clipboard, scrolled
  accessibility, physical cancellation and synchronized coverage remain open.
- **Power.** MC5 shared-radio power measurement is pending; wiring, supply
  isolation and measurement qualification remain open.

The [canonical index](design_docs/DOC_README.md) links exact receipts and open
criteria. Historical plan queues and older README summaries are not current
execution orders.

## Use

```sh
cargo test -p retinue      # or any host crate by name

# Flashing tool
cargo run -p linkboy -- list
cargo run -p linkboy -- flash <device> <package>

# Radio host app
cargo run -p signalman -- <port> [name] [bw_khz] [phy|rnode]

# Firmware (workspace member, not a default member)
cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf

# Desktop GUI (own workspace root)
cargo build --manifest-path apps/signalman-desktop/Cargo.toml
```

Run the [mixed-backend propagation test](testing/mixed-propagation/README.md) to compare TCP, raw Tulle and paired Tulle fetches in both directions.

## Portable survey scenes

`signalman::survey::scene::survey_scene` revalidates an admitted `Survey` and
exports a shared terrain/coverage recipe. GPX tracks become source guide paths,
with separate paths for separate segments; their endpoints are explicitly
labelled track samples. Radio identities from captures remain unplaced. No
observer/track association, RF time join, terrain inference or propagation
model is invented. The plane is marked unmeasured and has no coverage tiers.
The local equirectangular frame uses metres with north up and a 10 m margin;
its reference latitude and northwest corner are disclosed in the title.
Polar, antimeridian and over-budget extents are refused.

The exporter accepts a revalidated survey archive or bounded original files,
without opening a radio or obtaining credentials:

```sh
cargo run --features portable-scenes -p signalman --bin signalman-survey-scene -- \
  survey.json terrain.scene.json
cargo run --features portable-scenes -p signalman --bin signalman-survey-scene -- \
  --gpx track.gpx terrain.scene.json capture.json
```

The optional capture uses the existing survey-capture JSON schema. The direct
GPX form supplies no clock mappings; its join settings are unused. Both forms
refuse an existing output file. Graphshell's Portable scenes imports the result
with the shared renderer and inspection controls. These are application model
and file-export paths, not a new native survey panel or a radio qualification.

The portable-scene API is integrated in Mere's canonical `main` source.
The committed lockfile pins the shared scene cohort, so the commands above
need no temporary local source patches. Earlier local-cohort receipts remain
historical evidence; ordinary checks use `--locked` and `portable-scenes`.

## License

Mozilla Public License 2.0 ([LICENSE](LICENSE)) for the workspace, except
`crates/retinue`, `crates/outrider`, `crates/tulle` and `crates/radio-hand`.
Those are under the [Reticulum License](crates/retinue/LICENSE) (retinue and
outrider from 2026-10-05, tulle and radio-hand from 2026-10-09), so that the
reference implementations can be read and adapted into them. Releases through
retinue and outrider 0.2.0 and tulle 0.1.0 remain MPL-2.0.
Anything that links them carries the Reticulum License's notice and conditions
for those portions. That includes the apps and the firmware images, which are
therefore not GPLv3. See the
[decision record](design_docs/2026-10-05_reticulum_license_adoption.md).
Vendored third-party forks under `vendor/` (lora-phy, embedded-graphics,
embedded-graphics-core) keep their own MIT/Apache-2.0 terms.
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) aggregates derivations;
per-crate NOTICE and PROVENANCE files record specifics.

We (me and a gaggle of robots with any training data settings off) 
acknowledge Mark Qvist and the Reticulum contributors for the protocol,
its documentation, and the reference implementations. See the
[notices](THIRD_PARTY_NOTICES.md#reticulum).

## History

`tulle`, `sennet`, and `tucket` merged into this workspace on 2026-07-23 with
history preserved; their standalone repositories are archived. `sennet` and `tucket` were folded into
Signalman and removed from the workspace on 2026-11-14.

Retinue was first built from the public-domain Reticulum protocol using two v4s and a T114, 
the manual, and packet captures from the reference implementations, which were run as
black-box oracles like the other protocols. Comparative review of RNS source began on 2026-09-26.
Nothing was copied or translated before or after. My perspective is, if you can derive the protocol
from the wire, then you should do that as a control and functional baseline. The goal is 
verifiable interoperability with reference implementations, after all.

On 2026-10-05 retinue and outrider moved to the Reticulum License so that the reference 
implementations can be adapted into them; tulle and radio-hand followed on 2026-10-09 for the 
RNode host. Adaptations are listed in each crate's `NOTICE`. 
The rest of the workspace remains MPL-2.0, and RNode firmware source stays unread.

---

*This README was generated by AI and has/will be edited by the author upon
release.*


Signalman's Comms path also supports Meshtastic channel text and MeshCore
private text through the existing resident radio runtime. The Messages face
keeps protocol-specific addresses and observed delivery facts; a host supplies
its commissioned public journal and authorized runtime. The standalone face
can reopen an existing journal with `SIGNALMAN_RADIO_JOURNAL`. See R3 in
`design_docs/archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md` for the software qualification
and the separate physical and headed acceptance gates. R4 adds stock
Meshtastic Stream API and MeshCore USB companion connections: select the
protocol in Messages, enter its serial port, realm and public journal path,
then Connect. The device supplies its public identity and owns radio keys and
execution. Queue acceptance and protocol acknowledgements remain distinct;
MeshCore contact prefixes must resolve to a unique full key. See the same
plan's R4 section for startup settings and the software-only qualification.

## Local checks

Enable the commit checks with `prek install --allow-missing-config`; run
`prek run --all-files` alongside the existing local gates. Check definitions
live in `.pre-commit-config.yaml`, with source-size exclusions and exceptions
in `line-ceiling.toml`. Shared house style and rollout: `mere/design_docs/mere_docs/implementation_strategy/2026-10-30_house_style_plan.md`.
