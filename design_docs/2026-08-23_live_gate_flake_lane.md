# Live-gate flake lane

**Date:** 2026-08-23
**Status:** open lane, opened because the flake has a measurement but no owner
**Owns:** FLK1 through FLK5. The per-gate failure rates of the live RNS/LXMF
oracle gates, their mechanisms, and what a suite run is allowed to prove.

The live gate suites flake. This was recorded as a finding of the 2026-08-23
re-pin (`d93751b`) and measured per gate in `2e73365`, but no lane owned the
follow-up, so the measurement risked being read as a conclusion. It is not one.
This document takes ownership and states what would actually settle it.

The lane exists because the flake devalues every other receipt that quotes gate
counts. Until it is bounded, "twelve of twelve" is a snapshot of a coin-flipping
suite rather than a result.

## What is established

**The rate is per gate, not uniform, and the suite-level average hides that.**
The re-pin's "roughly 9% per gate" was inferred from suite-level results.
Measured directly, `interop_reqresp` failed 4 of 30 standalone runs, while
`interop_opportunistic_receive` went 32 of 32 interleaved and roughly 53
consecutive without a failure.

**One mechanism is identified and fixed, in the example rather than the
library.** Paired pass/fail logs showed `reqresp_interop.rs` exiting the instant
both done-conditions were met and dropping the interface underneath a peer that
had not read yet: in passing runs RNS logged receipt of the direction-2 response
*after* our socket closed; in failing runs it reported `None` while our own log
already said `ANSWERED_REQUEST`. `TcpInterface::send_raw` does `write_all` then
`flush`, so the library was never at fault. A 250 ms grace before return now
bookends the 250 ms the example already waits after `accept`.

**That fix changed the failure mode without changing the rate.** Four failures
in 30 runs before, four in 60 after; Fisher's exact p is about 0.44. The
teardown signature stopped appearing in captured failures.

**A sequence effect is ruled out.** `interop_reqresp` flakes at the same rate
run alone as inside the suite, so the suite's roughly-one-failure-per-run is
arithmetic over twelve gates rather than interference between them.

## What is not established

Two failure modes have no identified cause: `d2=false`, where retinue leaves the
receive loop before RNS's request arrives at all, and a collapse in which
direction 1 fails too. Both survive the teardown fix.

`interop_resource_recv` and `interop_ifac` were seen failing during the
alternating runs and have no baseline of their own. The remaining eight gates
have never been measured individually at all.

Two hypotheses were tested and failed. Raising RNS's log level to 7 made a
failure vanish (6 of 6), suggesting a timing race; forcing a rebuild before each
run, to reproduce the edit-then-test loop the first failures appeared in, did
not reproduce them (5 of 5).

## The sampling problem

This is the part that most needs stating, because it is why the lane cannot be
closed by another afternoon of runs.

Everything measured so far is consistent with a very wide range of true rates:

| observation | point estimate | 95% interval |
| --- | --- | --- |
| 4 of 30, before the fix | 13.3% | 1.2% to 25.5% |
| 4 of 60, after the fix | 6.7% | 0.4% to 13.0% |
| 8 of 90, pooled | 8.9% | 3.0% to 14.8% |

The pooled interval spans a factor of five. A five-run sample distinguishes
nothing whatever and must never be quoted as evidence about this.

Sizes the questions actually require, at the conventional 5% significance and
80% power:

| question | runs needed |
| --- | --- |
| separate a 13% rate from 7% | about 390 per arm |
| separate 13% from 9% | about 960 per arm |
| separate 13% from 5% | about 200 per arm |
| estimate one rate near 13% to plus or minus 5 points | about 175 |
| estimate it to plus or minus 3 points | about 480 |
| show a gate is quieter than 1%, given zero failures | 300 clean runs |

The practical reading: **30-run blocks cannot answer any question this lane
asks.** Campaigns belong in the hundreds, and the cheapest useful result is a
plus-or-minus-5-point estimate per gate at roughly 175 runs each, not a
high-precision figure for one gate.

## Sequence

1. **FLK1 — baseline `interop_reqresp` properly.** Run it standalone to a
   pre-declared n of at least 175, capturing logs for every failure and
   classifying each into `d2=false`, whole-exchange collapse, or a new mode.
   Declare n before starting; do not stop when the number looks good.
2. **FLK2 — baseline the rest.** The same treatment for the other eleven gates,
   `interop_resource_recv` and `interop_ifac` first since both have been seen
   failing. A gate with zero failures in 300 runs is bounded under 1% and can be
   set aside.
3. **FLK3 — explain `d2=false`.** Retinue leaves the receive loop before the
   request arrives. Determine whether the loop's exit condition is wrong or the
   request genuinely never arrives, from captures rather than from reasoning.
4. **FLK4 — explain the whole-exchange collapse.** The mode where direction 1
   fails too. Likelier to be a connection-establishment problem than a
   request/response one, but that is a guess and is labelled as one.
5. **FLK5 — set the evidence policy.** Once per-gate rates are bounded, state in
   the oracle README what a passing suite run is allowed to prove, and stop
   quoting bare "twelve of twelve" counts in receipts without a rate alongside.

FLK1 and FLK2 are mechanical and can run unattended. FLK3 and FLK4 depend on
FLK1's captured failures and should not start before there are enough of them to
classify.

## Done-conditions

- Every live gate has a failure rate with a stated interval and a declared
  sample size, or is bounded under 1% by 300 clean runs.
- Each observed failure mode is either explained or explicitly recorded as
  unexplained with its rate.
- The oracle README states what a suite run proves, and no receipt quotes a gate
  count without the rate context.
- No open claim in the repository rests on a bare clean-suite count.

## Explicitly out of scope

**The peer matrix is not in this lane and is not evidence about it.**
`peer_matrix.py` drives announce exchanges and transport forwarding only; it
never runs `interop_reqresp`, `interop_resource_recv`, or `interop_ifac`. The
seven clean runs behind the
[RNS 1.5.0 peer matrix receipt](2026-08-23_prns_peer_matrix_rns150_receipt.md)
are not weakened by this flake, and equally say nothing about it.

This lane does not own the wire-format question, the pin, or any RF behavior. It
owns only how often the live gates fail and why.
