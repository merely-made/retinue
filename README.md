# Retinue: managed cadre of mesh protocols

Embedded Rust implementations of three mesh protocols, a shared
radio interface layer, operator apps, and firmware that runs on my limited 
subset of hardware. 

So: what's the idea? Why all three? I wanna see if it's possible, using embassy,
to make the three more like channels that are triggered by a given announce, allowing
one node to configure a persona/address for each protocol and route traffic correctly. 
My mental picture: a retinue of protocols, working together. 

On the host side, this is possible with existing projects, but on the embedded side, 
doing it would mean forking C++, which I haven't had since college. So, Rust: the path 
of many such glitzy attempts, of late. Mine is comparatively drab: I prompt, I read, I type, 
I learn something new about Rust or embedded programming, I say "don't use this" when I don't 
know what it'll do, and *don't use this,* I don't know what it'll do.

I am pathologically honest: I use LLMs because I have come to truly enjoy leveraging my 
English degree in increasingly esoteric and elaborate ways, and I won't ever release or 
promote this implementation without demonstrated wire compatibility with reference 
implementations, at the very least. That's my goal, and I'm sticking to it.

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
| `sennet` | Meshtastic-compatible mesh. | published |
| `tucket` | MeshCore interop. | published |
| `radio-face`, `radio-hand`, `radio-mirror` | On-device UI, board runtime, and the UI's browser (wasm) realization. | workspace only |
| `retinue-sim` | In-process multi-node route-trace harness. | workspace only |
| `tinyssb-core` | Allocation-free tinySSB v0 feed verifier. | workspace only |

Apps: `linkboy` (inspects, plans and flashes firmware packages), `signalman`
(runs a household radio from a serial port), and `signalman-desktop` (its
desktop face, rooted in its own workspace). Firmware for Heltec V4 and Heltec
T114 boards is in `firmware/`, with flashable packages in `firmware/packages/`.

## Status (2026-10-06)

- **Releases.** retinue, outrider and postilion 0.2.0 and seneschal 0.1.0 were
  published on 2026-10-03 under MPL-2.0. retinue and outrider 0.3.0 is the
  first release under the Reticulum License; see [License](#license).
- **Reticulum.** The live oracle is pinned to RNS 1.5.6 and LXMF 1.2.0. The
  [repin receipt](testing/receipts/rns-1.5.6-repin/README.md) recorded every
  gate passing in at least one run. `interop_ifac` fails intermittently, from
  an RNS-side race that also occurs on 1.5.4. Outrider's 7 LXMF gates,
  12 Resource gates and 3 routing gates passed. RNS 1.5.7 was released
  2026-10-05 and has not been qualified yet. The 0.2.0
  [link lifecycle](design_docs/2026-10-02_node_link_lifecycle_plan.md) work
  landed: route learning, `open_link(now)`, link-request expiry, and
  own-echo and duplicate filtering. These results cover the measured
  local-TCP scopes; they are not full upstream parity.
- **Sennet and Tucket.** The [current receipt](testing/receipts/sennet-tucket-current/README.md)
  covers September 30 exchanges with identified stock radios. MeshCore 1.17.1
  passed endpoint traffic, ACKs, fallback and one-repeater routes at all three
  path widths. Meshtastic 2.7.26 passed bidirectional encrypted text,
  rebroadcast, node association and duplicate suppression. Maximum-size stock
  acceptance, multiple repeaters and broader roles remain open.
- **Firmware.** T114 bounded transport has an on-air receipt from August 13.
  The [firmware refresh](testing/receipts/firmware-refresh/README.md) and its
  [follow-up](testing/receipts/firmware-refresh/follow-up/README.md) installed
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

## License

Mozilla Public License 2.0 ([LICENSE](LICENSE)) for the workspace, except
`crates/retinue` and `crates/outrider`. Those two are under the
[Reticulum License](crates/retinue/LICENSE) from 2026-10-05, so that the
reference implementations can be read and adapted into them. Releases through
0.2.0 remain MPL-2.0; 0.3.0 is the first under the Reticulum License.
Anything that links them carries the Reticulum License's notice and conditions
for those portions. That includes the apps and the firmware images, which are
therefore not GPLv3. See the
[decision record](design_docs/2026-10-05_reticulum_license_adoption.md).
Vendored third-party forks under `vendor/` (lora-phy, embedded-graphics,
embedded-graphics-core) keep their own MIT/Apache-2.0 terms.
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) aggregates derivations;
per-crate NOTICE and PROVENANCE files record specifics.

We acknowledge Mark Qvist and the Reticulum contributors for the protocol,
its documentation, and the reference implementations. See the
[notices](THIRD_PARTY_NOTICES.md#reticulum).

## History

`tulle`, `sennet`, and `tucket` merged into this workspace on 2026-07-23 with
history preserved; their standalone repositories are archived.

Retinue was first built from the public-domain Reticulum protocol, the manual,
and packet captures from the reference implementations, which were run as
black-box oracles. Comparative review of RNS source began on 2026-09-26.
Nothing was copied or translated. On 2026-10-05 retinue and outrider moved to
the Reticulum License so that the reference implementations can be adapted
into them. Adaptations are listed in each crate's `NOTICE`. The rest of the
workspace remains MPL-2.0, and RNode firmware source stays unread.

---

*This README was generated by AI and has/will be edited by the author upon
release.*
