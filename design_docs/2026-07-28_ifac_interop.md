# IFAC interoperability and direct-PHY receipt

**Status: passed 2026-07-28.** R8 is complete.

## Boundary

IFAC is an interface envelope. `ifac::Ifac` authenticates and unmasks a frame
before `Packet::decode`; routing handles the recovered logical packet; each
egress interface signs and masks it again with that interface's credentials.
IFAC types and bytes do not enter link, Resource, or application models.

The same sans-I/O codec is used by standalone TCP, Endpoint TCP, raw
interfaces, and the Tulle radio pump. Its code length is configurable from 1
to 64 bytes. Complete-frame admission includes that overhead.

## Stock wire capture

`oracle/capture_ifac.py` ran pinned RNS 1.4.0 against a recording TCP listener.
It produced the deterministic fixtures `ifac_packet.bin` and
`ifac_packet.json`, then recomputed the credential derivation, signature
suffix, insertion point, and HKDF mask from public primitives.

The fixture is stable across recapture:

```text
SHA256(ifac_packet.bin)
9534C9E22797D7AE188441A22790F103F65890D111F656DCF787E59C6C0ED7C5
```

Retinue seals the logical fixture to the exact 47 stock bytes and opens those
bytes back to the exact 39-byte logical packet. Wrong credentials and a
one-bit mutation return `BadIfac`.

## Mixed-runtime gate

Command:

```powershell
.\oracle\.venv\Scripts\python.exe -u .\oracle\interop_ifac.py
```

Receipt:

```text
retinue -> RNS IFAC: PASS
RNS -> retinue IFAC: PASS
IFAC INTEROP: PASS
```

RNS accepted Retinue's authenticated announce through its own announce
handler. Retinue authenticated, decoded, and validated RNS's announce on the
same TCP connection. The ordinary open-interface `interop_r1.py` gate was
rerun afterward and also passed both directions.

## Direct-PHY receipt

Connected hardware:

- COM6: Heltec WiFi LoRa 32 V4, Tulle direct-PHY firmware
- COM10: Heltec T114, Tulle direct-PHY firmware

Command:

```powershell
cargo run -p retinue --features tulle-radio --example direct_phy_bytes -- `
  COM6 COM10 crates/retinue/tests/fixtures/ifac_packet.bin `
  C:\t\graphshell-target\ifac-rf-output.bin `
  250 120 retinue-ifac-rf headed-proof
```

Receipt:

```text
radios online: COM6=receiver, COM10=sender
interface: IFAC authenticated with logical MTU 247
discovery: byte-carriage destination announced over direct PHY
carriage: 47 bytes passed byte-exact in 4.1s
RETINUE DIRECT-PHY BYTES HEADED PASSED
```

Input and output SHA-256 were both
`9534C9E22797D7AE188441A22790F103F65890D111F656DCF787E59C6C0ED7C5`.
The 255-byte physical frame cap becomes a 247-byte logical link MTU under the
eight-byte IFAC. Queue admission also checks the final 255-byte carrier size,
so it cannot issue a local receipt for an unsealable packet.

## Regression receipts

- `cargo test -p retinue --all-features`: 95 library tests, 64 integration
  tests, and one doctest passed.
- `cargo test -p retinue --lib --no-default-features`: 87 passed.
- `cargo clippy -p retinue --all-targets --all-features -- -D warnings`:
  passed.
- `cargo fmt --all -- --check`: passed.

This proves protocol bytes, rejection, per-egress reapplication, stock
interoperability, and real RF carriage. It does not measure the eight-byte
airtime or power cost; that remains part of the deferred meter work.
