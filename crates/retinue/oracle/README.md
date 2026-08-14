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

## Setup

```sh
py -m venv .venv
./.venv/Scripts/python.exe -m pip install -r requirements.txt
```

`requirements.txt` pins `rns==1.4.2`, Retinue's current compatibility target. Re-pin
deliberately, not on every upstream release. Re-pinned 2026-08-06 from 1.4.0 (which carried
the 1.3.9 `rnsh` security fix); the twelve live gates pass on it, and re-capturing
`ifac_packet.bin` produced bytes identical to the 1.3.8 original, so the IFAC wire has not
moved across 1.3.8 → 1.4.2. The committed fixture corpus otherwise stays labelled 1.3.8
because those files are historical byte observations; current compatibility is established
by the live gates below.

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

- Retinue ↔ stock RNS 1.4.2, as the control pairing;
- Retinue ↔ Prns at the pinned H8 commit;
- Prns ↔ stock RNS 1.4.2; and
- stock RNS and Prns transport forwarding, independently, so O-10 compares
  their forwarded hop byte rather than relying on donor-source interpretation.

Create a clean peer worktree, build only its daemon, then run the matrix. On
this checkout Cargo uses `C:\t\graphshell-target`; prebuilt Retinue examples
there are used when present so an unrelated workspace build cannot stall a
receipt. Otherwise the driver falls back to `cargo run`.

```powershell
$peer = "$env:TEMP\retinue-peer-prns-72b6b30d"
git -C C:\Users\mark_\Code\repos\Prns worktree add --detach $peer 72b6b30d27cac910ce20d370e1dc711fe9b95955
$env:CARGO_TARGET_DIR = "$env:TEMP\retinue-peer-prns-target"
cargo build --manifest-path "$peer\prnsd\Cargo.toml" -p prnsd --no-default-features --features tokio-host,observability

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
| `requirements.txt` | the current live-oracle pin: `rns==1.4.2` |
| `run_live.py` | the complete eleven-gate mixed-runtime matrix |
| `capture.py` | R0 fixtures: identity vector, announces, negatives, a token |
| `capture_tcp.py` | R1 fixtures: the raw TCP stream, and the framing rules |
| `interop_r1.py` | the R1 live two-way announce gate |
| `capture_link.py` | link handshake probe: the trailer and link-id derivation |
| `link_crypto_probe.py` | pins the link key derivation by decrypting real RNS link traffic |
| `capture_link_session.py` | R3 fixtures: a deterministic captured link session |
| `interop_link.py` | the R3 live encrypted-link gate |
| `.venv/` | gitignored |

The link-crypto probes are worth a note on method. `link_crypto_probe.py` acts as a link
initiator with a *fixed* ephemeral secret against a real RNS responder. Because we know our
own secret and RNS's ephemeral public arrives in the proof, the ECDH shared secret is
computable, so we can derive the session key ourselves and prove it by having RNS decrypt
data we encrypted, and by decrypting RNS's reply. That, plus RNS's own
`Link.link_id_from_lr_packet` / `mode_from_lr_packet` helpers as a cross-check, pinned the
entire link layer before a line of Rust was written.
