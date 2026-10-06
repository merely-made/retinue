# RNS 1.5.7 and LXMF 1.2.0 live qualification

**Date:** October 6, 2026. **Candidate pins:** `rns==1.5.7`, `lxmf==1.2.0`.
**Baseline:** Retinue `a63f2fd` (retinue and outrider 0.3.0), plus the harness
changes committed with this receipt (see Script findings).
**Supersedes:** [the 1.5.6 repin](../rns-1.5.6-repin/README.md), within the
scopes measured below.
**Host:** macOS 15 (Darwin 24.6.0), x86_64. The 1.5.4 and 1.5.6 receipts ran on
Windows.

## Acquisition and method

RNS 1.5.7 was published to PyPI on 2026-10-05. As of 2026-10-06 it is not
tagged on GitHub, where `master` is still `e40191b`. Mark asked to repin on the
latest release. LXMF 1.2.0 is still the latest LXMF.

The wheels were downloaded from PyPI with `pip download --no-deps`, and their
SHA-256 was checked against PyPI's published digests:

| Wheel | SHA-256 | Matches PyPI |
|---|---|---|
| `rns-1.5.7-py3-none-any.whl` | `0e4aaf8cc47f770da34572d2215b45fea900e104b009551600eb9741a41faddf` | yes |
| `lxmf-1.2.0-py3-none-any.whl` | `805f053e61082f585efc6d09beffd039fd921174c5e1547a2e690bc479d75b85` | yes |
| `cryptography-48.0.1-cp39-abi3-macosx_10_9_universal2.whl` | `4fdc69f8e4316bcf0c8c8ec1f26f285d12e8142d88d96c876a59a03be3f6ae67` | yes |
| `cffi-2.0.0-cp313-cp313-macosx_10_13_x86_64.whl` | `00bdf7acc5f795150faa6957054fbbca2439db2f775ce831222b66f192f03beb` | yes |
| `pycparser-2.23-py3-none-any.whl` | `e5c6e8d3fbad53479cab09ac03729e0a9faf2bee3db8208a550daf5af81a5934` | yes |
| `pyserial-3.5-py2.py3-none-any.whl` | `c4451db6ba391ca6ca299fb3ec7bae67a5c55dde170964c7a14ceefec02f2cf0` | yes |

The interpreter is CPython 3.13.16 from python-build-standalone release
`20261003`. The archive is
`cpython-3.13.16+20261003-x86_64-apple-darwin-install_only_stripped.tar.gz`,
SHA-256 `b4dad38ba6a344555ccb71a1b08caad0a6c0dda88c5803658bc95bd7f04e9f5c`,
which matches GitHub's published digest. A fresh venv was built from it, and
the wheels were installed with
`pip install --no-index --find-links <those files>`.
[install.json](install.json) is pip's report, and
[environment.txt](environment.txt) is the resulting `pip freeze`. `pip check`
reports no broken requirements.

This environment differs from the 1.5.6 receipt's in four places:

- **Python:** 3.13.16 here; it was 3.14.2.
- **cryptography:** 48.0.1 here; it was 49.0.0. Releases 49 and later publish
  no x86_64 macOS wheels.
- **cffi:** 2.0.0 here; it was 2.1.0.
- **pycparser:** 2.23 here; it was 3.0.

These are package-hash identifications, not verified publisher signatures. The
drivers use public reference APIs and CLIs and observe output and wire
behaviour. No RNS implementation source was read for this receipt.

[run.py](run.py) is the 1.5.6 runner with only its version assertion and
default raw-output path changed. Every lane ran with `CARGO_TARGET_DIR` set to
the repository's own `target/`.

## Results

| Lane | Gates | Result | Time |
|---|---|---|---|
| [live](live/) | 13 | all pass, in a single run | 134 s |
| [outrider](outrider/) | 7 | all pass, on LXMF 1.2.0 | 132 s |
| [resource](resource/) | 12 | all pass | 174 s |
| [routing](routing/) | 3 | all pass | 413 s |

**`interop_ifac`** passed here and in the preliminary Python 3.9 run below. The
RNS-side `'TCPClientInterface' object has no attribute 'ifac_size'` race
recorded on 1.5.4 and 1.5.6 was not observed. Run alone 12 more times on 1.5.7,
it passed all 12, with no `ifac_size` error in any log (logs in
[ifac-repeat/](ifac-repeat/)). That makes 14 clean runs, against 4 failures in
12 on 1.5.6 and 1 in 12 on 1.5.4. The earlier failures were on the Windows
host, so this does not tell an RNS fix apart from a host timing difference. The
race stays on the held list until a Windows run or an upstream change settles
it.

**Routing is unchanged from 1.5.6.**

- [route-comparison.json](route-comparison.json) and
  [same-blob-comparison.json](same-blob-comparison.json) compare the 1.5.6 and
  1.5.7 results on the fields the earlier receipts compared. They were produced
  by [compare_routes.py](compare_routes.py).
- All 72 route cells and all 6 same-blob cells are equal, and every
  measurement is valid.
- The three timebase probes in [timebase-result.json](timebase-result.json)
  give answers identical to 1.5.6 once versions, wall-clock times and paths are
  normalized.
- [routing-raw-manifest.json](routing-raw-manifest.json) hashes the 1,218 raw
  files that remain in the ignored `validation/results/rns-1.5.7-repin/`.

`compare_routes.py` reproduces both 1.5.6 comparisons from the committed files,
with one exception. The 1.5.6 receipt records `new_sha256` `d031fc3c...` for its
route result. That is the hash of a CRLF working copy; the committed file is
LF.

## Preliminary runs on Python 3.9 ([py39/](py39/))

The first attempt used the system Python 3.9.6. Its venv held the same RNS,
LXMF and cryptography wheels, with cffi 2.0.0 (cp39) and typing_extensions
4.16.0, which cryptography needs below Python 3.11. Its pip freeze is
[py39/environment.txt](py39/environment.txt).

- **live:** 13/13, in a single run.
- **resource:** 12/12.
- **outrider:** 5/7. The two propagation gates failed because their scripts
  looked for `lxmd.exe` (see Script findings). After the fix,
  [outrider-after-lxmd](py39/outrider-after-lxmd/) passed 7/7.
- **routing:** 1/3. `route-full` and `same-blob` failed to import, because
  `peer_matrix.py` uses `datetime.UTC`, which needs Python 3.11. With a
  temporary shim, [routing-after-utc](py39/routing-after-utc/) failed again: the
  recording relay's `except TimeoutError` does not catch `socket.timeout`
  before Python 3.10.

The harness therefore needs Python 3.11 or later. The shim was reverted, and
every lane was rerun on 3.13 above. The 3.9 results are kept as evidence, not
as the qualification.

## The corroboration re-checks ([link-echo/](link-echo/))

V1's scripts ([the 1.5.4 corroboration](../rns-1.5.4-link-echo-corroboration/README.md))
were run from a scratch copy against 1.5.7, with `RNS_ORACLE_PYTHON` set to the
3.13 venv, so the 1.5.4 evidence was not overwritten. Their outputs are here.
The results match 1.5.6. The link-echo summary is equal key for key, and the
announce-echo summary differs only in the per-run random identities.

- **Link-request timeout.** It is unchanged: 12.002, 18.002, 24.009, and
  30.002 s at 0-3 relays, and 12.072 and 24.066 s at 62,500 bps. Retinue's
  deadline (N3) and airtime allowance (N8) still match. Established-link
  controls came up in 0.007 s (1 relay) and 0.006 s (3 relays).
- **Own echoed link data** is still filtered (C3).
- **Own echoed Channel message: the RNS defect persists.** A's own sequence-0
  Channel message, echoed by the relay, is delivered to A as if from B, and B's
  genuine sequence 0 is then lost (C4: `A_received_own_Ac0` 1,
  `A_received_Bc0` 0). Retinue filters this (N10).
- **Own echoed announce** is unchanged. RNS still records its own identity and
  app_data in `known_destinations`, with no path, handler event, or rebroadcast.
- **TCP below 62,500 bps: the RNS defect persists.** At 1,000 bps, inbound
  frames raise `unsupported operand type(s) for +: 'NoneType' and 'int'`, and no
  path is learned (`q1_bitrate1000_stdout.txt`,
  `q1_unanswered_r0_bitrate1000_157.jsonl`).

These two defects were held, not reported, under mer3ly's Ruling 53. They are
present in 1.5.7.

## Script findings

- `crates/retinue/oracle/interop_reliable_stream.py` hard-required RNS 1.5.6.
  Its guard moves to 1.5.7 with this repin.
- Five outrider oracle scripts hardcoded `lxmd.exe`, so they failed off
  Windows. They now use `lxmd` there; behaviour on Windows is unchanged. The
  five are `interop_propagation_receive.py`, `interop_propagation_stock.py`,
  `capture_propagation_announce.py`, `capture_propagation_fetch_response.py`
  and `capture_propagation_large_fetch_response.py`.
- The oracle harness requires Python 3.11 or later. `peer_matrix.py` imports
  `datetime.UTC`, and more than twenty scripts catch socket timeouts as
  `TimeoutError`, which relies on Python 3.10's alias. The scripts are
  unchanged; build the oracle venv on 3.11 or later.
- `capture_firmware_ifac.py` and `capture_signed_artifact.py` are unchanged,
  for the reasons the 1.5.6 receipt gives.

## Reproduction

```
pip download --no-deps rns==1.5.7 lxmf==1.2.0     # then verify the SHA-256 above
python3.13 -m venv <venv>
<venv>/bin/pip install --no-index --find-links <dir> rns==1.5.7 lxmf==1.2.0
cargo build -p retinue -p outrider --examples --locked
set CARGO_TARGET_DIR to the repository's target/
<venv>/bin/python -u testing/receipts/rns-1.5.7-repin/run.py <live|outrider|resource|routing> --output <new dir>
python3 testing/receipts/rns-1.5.7-repin/compare_routes.py <1.5.6 result> <1.5.7 result> <output.json>
```

Run each lane with the oracle venv's Python, one at a time. Each gate has a
1,200-second limit, and existing outputs are refused.

## Open

- The pin moves to `rns==1.5.7` and `lxmf==1.2.0` with this receipt.
- This is the first receipt from a macOS host. The Windows host has not been
  rerun on 1.5.7.
- The open gates listed in the 1.5.4 receipt are unchanged. They include
  firmware IFAC, queue saturation, natural route expiry, and physical
  requalification.
