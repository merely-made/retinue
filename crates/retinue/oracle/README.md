# The oracle

The Python reference implementation of Reticulum, used as a **black-box interoperability
oracle**: we run it, drive it through its public API, and record the bytes it produces. We
never read its source.

That discipline is not squeamishness. Two reasons:

1. **Licensing.** RNS is under the Reticulum License, reproduced verbatim in
   [`RETICULUM_LICENSE`](RETICULUM_LICENSE) beside this file — a modified MIT whose added
   clauses (no harmful systems, no AI training datasets) must not attach to retinue's own
   code, because retinue is MPL-2.0 and published as such, and a use restriction cannot ride
   on MPL-2.0 code. The clean-room boundary is what keeps that true: retinue is derived from
   the public-domain protocol specification, from the MIT-licensed Beechat `reticulum`
   crate, and from bytes observed on the wire. Nothing else. The license's terms are
   honored for what this directory *does* use RNS for — local black-box interoperability
   testing, never redistributed from here.
2. **It keeps us honest.** Reading an implementation invites copying its bugs and its
   accidents. Observing its output forces every question to be answered by what actually
   goes on the wire. This paid for itself immediately: Beechat, the readable Rust
   implementation, turns out to be wrong in two places that only wire observation could
   have caught (see below).

Reading RNS source is therefore forbidden. Running it, calling its documented API,
inspecting its public constants at runtime, and reading its output are all fine.

The same implementation boundary applies to `source-derived-peer` projects named by the
permissive compatibility survey: their released behavior may motivate and serve a
black-box probe, but their implementation does not shape Retinue code. In particular,
microReticulum's `/*p`, `//p`, and `//z` comment blocks appear to reproduce restricted
Python reference source. Do not read or quote those blocks, regardless of the repository's
root licence. microReticulum remains a peer/probe lead rather than a donor.

## Setup

```sh
py -m venv .venv
./.venv/Scripts/python.exe -m pip install -r requirements.txt
```

`requirements.txt` pins `rns==1.5.2`, Retinue's current compatibility target, and
`lxmf==1.1.1`, which the Outrider oracle drives. Re-pin deliberately, not on every upstream
release.

Re-pinned 2026-08-29 from RNS 1.5.0 to 1.5.2 while holding LXMF at 1.1.1. One complete
stock-RNS live suite passed 12/12; four Resource-sensitive gates passed 12/12 across three
interleaved rounds; seven Outrider gates passed; the announce-timebase probe reproduced
P1/P2/P3; the corrected route probe passed 72/72 full cells and 6/6 packet-loop-isolated
same-blob cells; and the pinned-Prns H8 matrix passed 15/15 case executions across three
runs. The deterministic `ifac_packet.bin` was byte-identical between fresh 1.5.0 and 1.5.2
captures, and eight JSON records differed only in recorded version metadata after
normalisation. Other captures carry fresh entropy or require attached RNodes, so this is
deliberately narrower than the old 18/18 byte-identity claim. Queue saturation, I2P,
interface-discovery metadata, and physical RNode capture remain outside the measured scope.
See the [current re-pin receipt](../../../design_docs/2026-08-29_rns_152_repin_receipt.md).

The historical 2026-08-23 re-pin moved from `rns==1.4.2` / `lxmf==0.9.6` to RNS 1.5.0 /
LXMF 1.1.1. Eighteen deterministic fixtures re-captured byte-identically and one live suite
passed 12/12. The fixture result carried that claim because these live gates flake at a rate
this repository has not bounded; see the
[live-gate flake lane](../../../design_docs/2026-08-23_live_gate_flake_lane.md). The old
receipt reconstructed the release from package metadata and runtime constants because a
public release page was unavailable during that work. Upstream has since published one, so
the old document's “no public release notes” sentence is historical rather than current.

LXMF 1.1.1 did move a wire shape: the delivery announce grew a third element, a
supported-features array. Outrider refused it and had to be fixed; see
`design_docs/2026-08-23_rns_150_lxmf_111_repin_receipt.md`.

The committed fixture corpus otherwise stays labelled with the version that produced each
file, because those files are historical byte observations; current compatibility is
established by the live gates below.

Re-pinned 2026-08-06 from 1.4.0 (which carried the 1.3.9 `rnsh` security fix); at that
point re-capturing `ifac_packet.bin` produced bytes identical to the 1.3.8 original.

**On the 250 ms waits in the Rust probes, corrected 2026-08-06.** These were recorded as a
single `TCPClientInterface.ifac_size` initialization race in RNS. Measurement found two
different things wearing one name:

- **The IFAC half was ours.** `Ifac` checked the IFAC flag on the byte underneath the mask,
  where it is exclusive-or'd with a per-packet mask bit, so it refused about half of every
  peer's frames no matter how long anything waited. Fixed. `interop_ifac` now passes 5 of 5
  with no wait; before the fix, at the same zero wait, it passed 3 of 5.
- **The plain-TCP half is real, and is not about IFAC** — `interop_r1` carries no IFAC at
  all. With no wait it fails about one run in five, RNS closing the connection right after
  our announce: its `TCPClientInterface` drops a peer whose first frame arrives before it
  has finished connecting. `interop_tcp.rs` keeps its 250 ms, which gives 5 of 5, and now
  says why. A readiness signal would beat the clock there if it ever flakes again.

Retinue's production TCP interface has no such delay in either case.

## Capture

```sh
./.venv/Scripts/python.exe -u capture.py
```

`-u` matters. `RNS.exit()` hard-exits the process and will discard buffered stdout, so a
buffered run looks like it silently did nothing.

This writes `../tests/fixtures/`: the announce corpus, the negative (corrupted) announces,
an identity vector, an encrypted token, and `manifest.json` describing each one and the
facts it pins down.

The fixtures are **committed**. `cargo test` replays them and needs no Python, so CI stays
Python-free. The live oracle is a local gate, run when the wire format is in question.

`capture_ifac.py` separately emits a deterministic authenticated plain packet
(`ifac_packet.bin` and its JSON manifest). It states and falsifies derivation,
signature, placement, and mask hypotheses against stock RNS output.

## What the oracle settled

These were unanswerable from the manual and from Beechat, and a wrong guess on any of them
is a silent, total wire incompatibility.

- **Announces carry a ratchet, and Beechat cannot parse one.** A ratchet-enabled
  destination inserts a 32-byte X25519 public key between `rand_hash` and the signature,
  and signals it with **bit 5 of header byte 0** (the Context Flag). Beechat 0.1.0 models
  neither the flag nor the field, so it reads the ratchet where the signature should be and
  fails verification. Ratchets are off by default, which is the only reason a Beechat/RNS
  pairing appears to work at all.
- **The announce signature covers the destination hash, which is not in the payload.** It
  comes from the packet header. The signed message is the wire payload with the destination
  hash prepended and the signature spliced out, so `app_data` sits at a different offset in
  the signed form than on the wire.
- **The token is AES-256 with the signing key first.** `HKDF-SHA256(ikm=x25519_shared,
  salt=identity_hash, info=<empty>, len=64)`, then `sign_key = derived[0..32]`,
  `enc_key = derived[32..64]`. Established by decrypting a real RNS token against all four
  combinations of {AES-128, AES-256} x {sign-first, enc-first}; only one authenticates and
  decrypts. Beechat gets this right on its `PrivateIdentity` path and wrong on its
  `Identity` path, which hardcodes a 16-byte split that is only correct under a non-default
  feature.
- **`NAME_HASH_LENGTH` is 10 bytes**, which appears nowhere in the manual.
- **IFAC is a carrier envelope.** The oracle pinned credential hashing, the
  64-byte HKDF identity key, Ed25519 signature-suffix truncation, code placement,
  and the per-packet HKDF mask. The logical packet is recovered before packet
  decoding and is re-signed for every egress interface.

## The live interop gate

```sh
./.venv/Scripts/python.exe -u run_live.py
```

This runs all twelve live gates in isolated processes: open and IFAC-authenticated
announce, path resolution, links in
both roles, request/response, endpoint streaming, Resources in both directions (including
the 2.5 MB segmented cases), and transport routing. `interop_r1.py` is the first gate. It
starts retinue (`examples/interop_tcp.rs`), points a real RNS `TCPClientInterface` at it,
and checks **both** directions:

- **retinue -> RNS.** RNS's own announce handler accepts an announce retinue built,
  signed and framed. Reaching the handler at all means it passed RNS's signature
  validation.
- **RNS -> retinue.** retinue de-frames, decodes and validates RNS's announce over the
same socket.

`interop_ifac.py` repeats that receipt over a named, passphrase-protected
interface. RNS accepts Retinue's authenticated announce and Retinue unmasks,
verifies, and validates RNS's announce on the same connection.

Either direction failing means we are not wire-compatible, whatever the unit tests say.
This is a **local gate**, not CI: CI replays the committed fixtures instead.

## Peer matrix (H8)

`peer_matrix.py` adds the three-corner peer receipt from the work-lane map. It
launches a **clean detached** Prns worktree as an external `prnsd` process;
Retinue has no Prns dependency and the driver reads no Prns library API. Each
case uses a localhost recording relay, preserving both directional TCP byte
streams under `validation/results/` with SHA-256 digests.

The matrix covers:

- Retinue ↔ stock RNS 1.5.2, as the control pairing;
- Retinue ↔ Prns at the pinned H8 commit;
- Prns ↔ stock RNS 1.5.2; and
- stock RNS and Prns transport forwarding, independently, so O-10 compares
  their forwarded hop byte rather than relying on donor-source interpretation.

Create a clean peer worktree, build only its daemon, then run the matrix. On
this checkout Cargo uses `C:\t\graphshell-target`; prebuilt Retinue examples
there are used when present so an unrelated workspace build cannot stall a
receipt. Otherwise the driver falls back to `cargo run`.

The peer daemon **must be built from inside the peer worktree**, not from here.
Prns pins a 256 MiB Windows stack in its own `.cargo/config.toml`, and Cargo reads
that file relative to the working directory rather than to `--manifest-path`.

```powershell
$peer = "$env:TEMP\retinue-peer-prns-72b6b30d"
git -C C:\Users\mark_\Code\crates\prns worktree add --detach $peer 72b6b30d27cac910ce20d370e1dc711fe9b95955
$env:CARGO_TARGET_DIR = "$env:TEMP\retinue-peer-prns-target"
# Build from inside $peer.  Cargo resolves .cargo/config.toml from the working
# directory, not from --manifest-path, and Prns's own config carries
# link-arg=/STACK:268435456 for windows-msvc.  Built from anywhere else that
# setting is silently dropped, and the daemon overflows the default 1 MiB stack
# before it can parse an argument -- `prnsd --version` dies and the matrix
# aborts on its first peer call.
Push-Location $peer
cargo build --manifest-path "$peer\prnsd\Cargo.toml" -p prnsd --no-default-features --features tokio-host,observability
Pop-Location

.\.venv\Scripts\python.exe -u .\peer_matrix.py `
  --prns-root $peer `
  --prnsd "$env:CARGO_TARGET_DIR\debug\prnsd.exe"
```

The runner rejects a dirty or differently pinned Prns worktree. A pass is a
local TCP interoperability receipt, not an RF or range receipt. Its result
directory is intentionally ignored because it contains transient identities,
ports, raw captures, and exact clean-commit state.

## Files

| file | what |
| --- | --- |
| `requirements.txt` | the current live-oracle pin: `rns==1.5.2`, `lxmf==1.1.1` |
| `run_live.py` | the complete twelve-gate mixed-runtime matrix |
| `flake_census.py` | census one gate by failure **mode**, not rate; see below |
| `capture.py` | R0 fixtures: identity vector, announces, negatives, a token |
| `capture_tcp.py` | R1 fixtures: the raw TCP stream, and the framing rules |
| `interop_r1.py` | the R1 live two-way announce gate |
| `capture_link.py` | link handshake probe: the trailer and link-id derivation |
| `link_crypto_probe.py` | pins the link key derivation by decrypting real RNS link traffic |
| `capture_link_session.py` | R3 fixtures: a deterministic captured link session |
| `interop_link.py` | the R3 live encrypted-link gate |
| `probe_announce_timebase.py` | persistent P1/P2/P3 announce-timebase matrix |
| `probe_route_freshness.py` | natural transported P8 route/freshness matrix and same-blob diagnostic |
| `.venv/` | gitignored |

The link-crypto probes are worth a note on method. `link_crypto_probe.py` acts as a link
initiator with a *fixed* ephemeral secret against a real RNS responder. Because we know our
own secret and RNS's ephemeral public arrives in the proof, the ECDH shared secret is
computable, so we can derive the session key ourselves and prove it by having RNS decrypt
data we encrypted, and by decrypting RNS's reply. That, plus RNS's own
`Link.link_id_from_lr_packet` / `mode_from_lr_packet` helpers as a cross-check, pinned the
entire link layer before a line of Rust was written.

## Censusing a flaky gate

```sh
./.venv/Scripts/python.exe flake_census.py interop_reqresp.py 120
```

These gates flake, and the rate differs per gate: `interop_reqresp` was measured
at 4 failures in 30 standalone runs while `interop_opportunistic_receive` went 32
of 32. **A single suite run is therefore weak evidence, and a bare "twelve of
twelve" should not be quoted in a receipt without a rate beside it.**

Counting is the wrong instrument for chasing that. Separating a 13% failure rate
from a 7% one needs roughly 390 runs per arm, so the 30-run block is worse than
useless — it produces numbers that look like findings. On 2026-08-23 that error
cost an afternoon and a confident, wrong claim that RNS 1.5.0 had regressed
opportunistic delivery by 20%; the apparent effect was an artifact of running the
four arms as sequential blocks on a machine whose load drifted, and it vanished
when the arms were interleaved.

`flake_census.py` classifies instead. It runs one gate n times, fingerprints every
failure across its verdict lines and a handful of gate-agnostic signals, groups the
fingerprints into modes and keeps one exemplar log per mode. Seven classified
failures of `interop_reqresp` located three distinct bugs — a discarded inbound
link request, an announce lost to the gate's handler-registration window, and a
peer dropped during connection setup — that no number of counted runs would have
found. Each fix was then confirmed by its mode going to zero, which is a much
cheaper thing to establish than a rate.

Two cautions the tool now enforces. It **discards runs that died in the build**
rather than in the gate, because a shared target directory under heavy
parallelism manufactures stale-rlib failures that are not gate failures. And it
**records the concurrent rustc and cargo count** at the start and end of every
census: these gates are timing-sensitive localhost networking, so a census taken
during a build storm measures the machine as much as the gate, and two censuses
taken under different load cannot be pooled or compared.

Exemplar logs land in `census/`, which is not committed.

## Announce timebase probe

`probe_announce_timebase.py` is a clean-room, black-box probe at the current RNS 1.5.2 pin for
the P1/P2/P3 matrix. It creates each signed packet in a sender child, injects
it into a fresh receiver child, and reuses the receiver's persistent config
between cases. The post-shutdown `storage/destination_table` is authoritative;
`in_process_snapshot` is diagnostic only. A false answer is still a valid
measurement. Run it from this directory:

```powershell
.\.venv\Scripts\python.exe -u probe_announce_timebase.py
```

The current local, ignored receipt is
`validation/results/announce-timebase-20260830T024551Z/result.json`, SHA-256
`252c29b40dc4b972f1615935e434a8a1802cccf6f2db6ec641f4bae851b5fef0`. P1 rejected a
fresh nonce with an equal timebase at the same one-hop route, P2 rejected timebase `2`
after accepting `2^39`, and P3 accepted both `1` and `2^39`. All first sightings persisted
with valid packets. The historical RNS 1.5.0 receipt is
`validation/results/announce-timebase-final2/result.json`, SHA-256
`639dda1d1d4f8ef9128a6a4f4ceeda00444524a62c1da234c7c167d5a6ab1ac1`.

## Route/freshness probe

`probe_route_freshness.py` is the clean-room P8 probe. It sends public-API-generated,
signed Type-1 announces through natural stock-RNS transport chains, records the Type-2
frames at a persistent receiver, and treats the receiver's post-shutdown
`storage/destination_table` as authority. Better, equal, and worse paths are calibrated as
two, three, and four transport hops against a three-hop incumbent. Path-response rows are
seeded while the receiver is disconnected and then requested with public
`RNS.Transport.request_path`; context `0x0b` is never synthesised.
Before stage two, each terminal is connected to a passive drain until an observed quiet
window; every arm is then checked for every seeded candidate before requests begin. Stage
one also waits on live receiver acceptance events before shutdown. Each cell has a distinct
destination. Cells share one persistent receiver and one candidate
chain per hop relation; the result records that shared-global-state scope explicitly.

Run the smoke matrix, the full 72-cell matrix, and the packet-loop isolation diagnostic
from this directory:

```powershell
.\.venv\Scripts\python.exe -u probe_route_freshness.py --profile smoke
.\.venv\Scripts\python.exe -u probe_route_freshness.py --profile full
.\.venv\Scripts\python.exe -u probe_route_freshness.py --profile same-blob-diagnostic
```

The current ignored RNS 1.5.2 full receipt is
`validation/results/route-freshness-full-20260830T030952Z/result.json`, SHA-256
`14601b688fe72e1763e8d022915c468d1f9b164715bc47aee726050b905aaf39`. All 72 rows have a
publicly signature-validated forwarded Type-2 frame and calibrated hop relation. No
row has a matching signature-valid frame with an unexpected header type or context.
Ordinary and real path-response
contexts behave identically:

| incumbent state | candidate | measured outcome |
| --- | --- | --- |
| live | strictly newer timebase | admit at better, equal, or worse hops |
| live | equal or older timebase, including an exact-same blob | no observable admission |
| loaded expired | new blob with a strictly newer timebase | admit at better, equal, or worse hops |
| loaded expired | new blob with an equal or older timebase | admit only at worse hops |
| loaded expired | exact-same blob | no observable admission at any hop relation |

The historical RNS 1.5.0 full receipt is
`validation/results/route-freshness-full-20260826T211647Z/result.json`, SHA-256
`bcb83e38b9d840926f2ee3a7093a37877fa6f84e2d2c4ed1290c4290c2a17a38`.

The six exact-same ordinary rows could otherwise be hidden by RNS's packet-loop window.
The separate receipt moves the observed `packet_hashlist.raw` aside while preserving the
destination table, then reloads stock RNS and repeats live/expired by better/equal/worse.
The original list contained all six incumbent route packet hashes. The pre-candidate list
contained one reload-generated hash and none of those six. All six measurements remained
no-admission. The current RNS 1.5.2 result is
`validation/results/route-freshness-same-blob-diagnostic-20260830T030802Z/result.json`,
SHA-256 `d660ea18f6ce38d0029672d85829a257d25f53dd30ae2df9cec63fe2f6972550`.
The historical RNS 1.5.0 result is
`validation/results/route-freshness-same-blob-diagnostic-20260826T212136Z/result.json`,
SHA-256 `7b9680456492d7577b78fdd5b0007ad17934ebb966b50eb5549c2f2b83c269fc`.

The expired arm is deliberately named `loaded-expired-state`. The probe independently
decodes and byte-identically re-encodes the observed MessagePack table, changes only the
selected expiry `f64` values to the past, and reloads stock RNS. This measures admission
against loaded expired state, not the natural elapsed-expiry lifecycle.
