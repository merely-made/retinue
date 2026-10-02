# RNS 1.5.4 link timeout and echo corroboration (lane V1, 2026-10-02)

This is a black-box receipt against stock **RNS 1.5.4**. The version was confirmed
with `importlib.metadata.version('rns')`. It used the installed oracle venv at
`crates/retinue/oracle/.venv` (Python 3.14.2) in the main checkout. Nothing was
downloaded or installed.

Every RNS instance is a separate `node.py` process, driven only through the public
RNS API. Instances talk over localhost TCP, and relays have `enable_transport = Yes`.
`harness.py` holds the helpers. It does not import RNS. It runs an HDLC-aware TCP
proxy that logs every frame in both directions, and can withhold a frame or inject
one. The frame labels (type, hops, destination, context, packet hash) come from
Retinue's own wire reference.

A comparative source review was run alongside this receipt. Its citations go to the
coordinating session; they are not repeated here. No RNS code was copied or
translated.

## Commands

From this directory:

```powershell
$py = "C:\Users\mark_\Code\repos\retinue\crates\retinue\oracle\.venv\Scripts\python.exe"
& $py -u q1_link_timeout.py --reps 2      # unanswered link request at 0-3 relays
& $py -u q1_link_timeout.py --tail-only   # bitrate cells and established-link controls
& $py -u q2_link_echo.py                  # plus --tag q2_link_echo_rep1
& $py -u q3_announce_echo.py              # plus --tag q3_announce_echo_suppressed --suppress-natural
```

`RNS_ORACLE_PYTHON` overrides the interpreter path.

## Outputs (`results/`)

Each `*.jsonl` file is one run. It interleaves node events (wall-clock `t`),
commands, every proxy frame with its raw hex, injections, and a final `result` record.

| file | shows |
| --- | --- |
| `q1_summary.json` | All Q1 result records. |
| `q1_unanswered_r{0..3}_rep{0,1}.jsonl` | A's request to a destination that refuses links (`accepts_links(False)`). Measures the wait from `RNS.Link(...)` to the closed callback, the LINKREQUEST frames seen on the wire, and whether any proof came back. |
| `q1_unanswered_r{0,2}_bitrate62500.jsonl` | Same, with `bitrate = 62500` on A's interface. |
| `q1_FAILED_r0_bitrate1000_hwmtu_none.jsonl`, `q1_stdout.txt` | The first bitrate attempt, at 1000 bps. RNS raised `NoneType + int` on every inbound frame and A never learned a path. Below 62,500 bps an auto-MTU interface gets `HW_MTU = None`. This is an RNS defect outside this lane, and the retry used 62,500 bps. |
| `q1_controls_r{1,3}.jsonl` | Positive control: the link establishes. Then a remote teardown, and a silent loss where B is killed after establishment. |
| `q2_link_echo*.jsonl` / `_summary.json` | A -> P1 -> R -> P2 -> B, run twice. See the cases below. |
| `q3_announce_echo*.jsonl` / `_summary.json` | C -> P3 -> A(transport) -> P1 -> R -> P2 -> B. One arm leaves R's natural echo in place; the other suppresses it. |

## Results

**Q1.** The observed wait was:

- 12.001 s at 0 relays (both runs)
- 18.001 and 18.002 s at 1 relay
- 24.001 s at 2 relays (both runs)
- 30.001 and 30.024 s at 3 relays

Each run sent exactly one LINKREQUEST frame on the wire, and no proof came back.
At 62,500 bps the waits were 12.079 s (0 relays) and 24.071 s (2 relays), against
RNS-computed values of 12.064 and 24.064 s.

On expiry the closed callback fired with `status` = CLOSED (4) and
`teardown_reason` = TIMEOUT (1). `activated_at` was None, and the established
callback never fired.

In the controls, a remote teardown gave DESTINATION_CLOSED (3). A silent loss after
establishment gave **TIMEOUT (1)**, roughly 14 s after the kill, with `activated_at`
set.

**Q2.** Both runs gave identical results.

- C0: genuine data is delivered in both directions.
- C1: a withheld B frame, injected later, is delivered once. This proves the
  injection path works.
- C2: an identical duplicate is not delivered, and neither is one with hops+1.
- C3: R's retransmission of A's own packet carries hops 1 and the same packet hash.
  Injected into A, it is not delivered. The verbatim echo is not delivered either,
  and the link keeps working afterwards.
- C4 (Channel context): R's retransmission of A's own Channel message (sequence 0)
  **is delivered to A's application** and A sends a proof for it. B's genuine
  sequence-0 message is then never delivered, while B's sequence 1 is. A second
  copy of the echo is not delivered.

**Q3.** Positive control: B's announce makes A record B (`recall_app_data`
returns `B-app`), learn a 2-hop path, fire its handler, and re-broadcast to C.

R echoes A's own announce back to A over TCP without any injection. With that
echo suppressed, A's own destination does not appear in `known_destinations`.
After one injected echo it does, and `recall_app_data` returns `A-app`.

In every arm, A learns no path to itself, fires no handler and sends no
re-broadcast.
