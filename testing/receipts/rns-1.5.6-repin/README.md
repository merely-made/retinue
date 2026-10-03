# RNS 1.5.6 and LXMF 1.2.0 live qualification

**Date:** October 3, 2026. **Candidate pins:** `rns==1.5.6`, `lxmf==1.2.0`.
**Baseline:** Retinue `f149dea318bebcbc15daa6be98819ed2b64607df`, clean, with
retinue 0.2.0 (the link-lifecycle work in
[the plan](../../../design_docs/2026-10-02_node_link_lifecycle_plan.md)).
**Supersedes:** [the 1.5.4 repin](../rns-1.5.4-repin/README.md), within the
scopes measured below. `requirements.txt` moves only when the pin is ruled
(see Open).

## Acquisition and method

RNS 1.5.6 was published on 2026-10-02 and superseded 1.5.5, published
2026-09-29; Mark ruled that the target is 1.5.6. LXMF 1.2.0 (2026-09-30,
`requires rns>=1.5.5`) moves with it.

The wheels were downloaded from PyPI with `pip download --no-deps`, and their
SHA-256 was checked against PyPI's published digests:

| Wheel | SHA-256 | Matches PyPI |
|---|---|---|
| `rns-1.5.6-py3-none-any.whl` | `975a646836749560f4553e4cc82d7dc9774c6c79c8beef08357264e1d8443626` | yes |
| `lxmf-1.2.0-py3-none-any.whl` | `805f053e61082f585efc6d09beffd039fd921174c5e1547a2e690bc479d75b85` | yes |

They were installed into the existing oracle venv with
`pip install --no-index --find-links <those files>`. [install.json](install.json)
is pip's report, and [environment.txt](environment.txt) is the resulting
`pip freeze`. cryptography (49.0.0) and pyserial (3.5) are unchanged.
`pip check` reports no broken requirements.

These are package-hash identification only, not verified publisher signatures.
The drivers use public reference APIs and CLIs and observe output and wire
behaviour. No RNS or Prns implementation code was copied or translated.

[run.py](run.py) is the 1.5.4 runner with only its version assertions and
default raw-output path changed. Every lane ran with `CARGO_TARGET_DIR` set to
the repository's own `target/`.

## Results

| Lane | Gates | Result | Time |
|---|---|---|---|
| [live](live/) | 13 | 12 pass. `interop_reliable_stream` refused to run: its guard required RNS 1.5.4 | 150 s |
| [live-after-guard](live-after-guard/) | 13 | 12 pass. `interop_reliable_stream` needed its prebuilt example and found none at its `C:\t\...` fallback | 94 s |
| [live-after-target](live-after-target/) | 13 | 12 pass, including `interop_reliable_stream`. `interop_ifac` failed (see IFAC) | 200 s |
| [outrider](outrider/) | 7 | all pass, on LXMF 1.2.0 | 152 s |
| [resource](resource/) | 12 | all pass | 94 s |
| [routing](routing/) | 3 | all pass | 401 s |

On `live`, the 1.5.4 receipt ran 12 gates. `interop_reliable_stream` was added
after it, and its guard is moved to 1.5.6 by this repin. The other 12 gates
launch `cargo run --example`, so they always build current code.

Routing is unchanged from 1.5.4.
- [route-comparison.json](route-comparison.json) and
  [same-blob-comparison.json](same-blob-comparison.json) compare the 1.5.4 and
  1.5.6 results on the fields the 1.5.4 receipt compared.
- All 72 route cells and all 6 same-blob cells are equal, and every measurement
  is valid.
- [routing-raw-manifest.json](routing-raw-manifest.json) hashes the 1,218 raw
  files that remain in the ignored `validation/results/rns-1.5.6-repin/`.

## IFAC: an intermittent RNS-side failure

`interop_ifac` passed in the first two live runs and failed in the third. Run
alone 12 times on 1.5.6, it passed 8 and failed 4 (logs in
[ifac-repeat/](ifac-repeat/)).

Every failure has the same RNS-side exception on the IFAC-configured TCP
interface: `'TCPClientInterface' object has no attribute 'ifac_size'`. RNS then
tears the interface down, and both directions fail. The passing runs show no
such error.

Whether this is new in 1.5.6 is **unmeasured**. The 1.5.4 receipt ran this gate
twice, and both runs passed. A comparison needs 1.5.4 reinstalled in a scratch
environment.

## The corroboration re-checks ([link-echo/](link-echo/))

V1's scripts ([the 1.5.4 corroboration](../rns-1.5.4-link-echo-corroboration/README.md))
were run from a scratch copy against 1.5.6, so the 1.5.4 evidence was not
overwritten. Their outputs are here.

- **Link-request timeout.** It is unchanged: 12.001, 18.001, 24.001, and 30.001 s
  at 0-3 relays, and 12.066 and 24.065 s at 62,500 bps. Retinue's deadline (N3)
  and airtime allowance (N8) still match.
- **Own echoed link data** is still filtered (C3).
- **Own echoed Channel message: the RNS defect persists.** A's own sequence-0
  Channel message, echoed by the relay, is delivered to A as if from B, and B's
  genuine sequence 0 is then lost (C4: `A_received_own_Ac0` 1,
  `A_received_Bc0` 0). Retinue filters this (N10).
- **Own echoed announce** is unchanged. RNS still records its own identity and
  app_data in `known_destinations`, with no path, handler event, or rebroadcast.
- **TCP below 62,500 bps: the RNS defect persists.** At 1,000 bps, inbound
  frames raise `unsupported operand type(s) for +: 'NoneType' and 'int'`, and no
  path is learned (`q1_bitrate1000_stdout.txt`).

These two defects were held, not reported, under mer3ly's Ruling 53. They are
present in 1.5.6.

## Script findings

- `crates/retinue/oracle/interop_reliable_stream.py` hard-required RNS 1.5.4.
  Its guard moves to 1.5.6 with this repin.
- `crates/retinue/oracle/capture_firmware_ifac.py` requires 1.5.4 by design,
  because it produces 1.5.4-labelled fixtures. It is unchanged.
- `crates/retinue/oracle/capture_signed_artifact.py` names every non-1.5.2
  capture `rns_signed_artifact_1_5_4.json`. It is not run by any lane, but run
  on 1.5.6 it would mislabel its output. It is unchanged here.

## Reproduction

```
pip download --no-deps rns==1.5.6 lxmf==1.2.0     # then verify the SHA-256 above
pip install --no-index --find-links <dir> rns==1.5.6 lxmf==1.2.0
cargo build -p retinue -p outrider --examples --locked
set CARGO_TARGET_DIR to the repository's target/
python -u testing/receipts/rns-1.5.6-repin/run.py <live|outrider|resource|routing> --output <new dir>
```

Run each lane with the oracle venv's Python, one at a time. Each gate has a
1,200-second limit, and existing outputs are refused.

## Open

- The pin itself: `requirements.txt` stays at 1.5.4 and 1.1.1 until the IFAC
  finding is ruled.
- Whether the IFAC failure predates 1.5.6.
- The open gates listed in the 1.5.4 receipt are unchanged. They include
  firmware IFAC, queue saturation, natural route expiry, and physical
  requalification.
