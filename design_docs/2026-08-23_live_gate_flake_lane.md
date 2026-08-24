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

## FLK3 and FLK4 are closed: three more mechanisms, found by classifying

**Amended 2026-08-23 by the re-pin session, which owned this work already.** The
two modes recorded below as having no identified cause now have three
mechanisms between them, all read off code paths rather than inferred, and all
three modes have gone to zero across 120-run censuses.

The method matters as much as the result, because it is the answer to the
sampling problem this document states so well. Rate comparison is the expensive
instrument: separating 13% from 7% needs about 390 runs an arm. Classifying
failures by signature is the cheap one, and it is strictly more informative --
**seven classified failures located three distinct bugs that no number of
counted runs would have found.** `scratchpad/flake_census.py` runs a gate n
times, fingerprints every failure across fourteen signals, groups them into
modes and keeps one exemplar log per mode. FLK1 and FLK2 should adopt it: a gate
bounded under 1% by 300 clean runs is worth more when you also know the shapes
of the failures that did occur.

**FLK4 -- whole-exchange collapse. The lane's guess was right: it is connection
establishment, not request/response.** The signature is `TIMEOUT proof` with
nothing else whatsoever -- no `SENT_REQUEST`, no `DONE`, RNS never sees the
`/svc` announce, and there is no traceback, IO error or timeout anywhere. RNS's
`TCPClientInterface` drops a peer whose first frame arrives before it has
finished connecting, the same behaviour `oracle/README.md` already records
behind `interop_r1`'s 250 ms wait. A peer dropped that way **stays** dropped:
adding a loop that resends the announce and link request every second for ten
seconds does not revive it, which is the evidence that the frames are being
discarded rather than lost. The only lever is the post-accept settle, raised
250 ms -> 750 ms. **That number is the weakest thing in this amendment** -- a
guess at a distribution nobody has characterised, of exactly the kind that left
`interop_r1` carrying a superstitious sleep for months. This mode is the sole
survivor of the final census, at 1 in 120, and deserves its own FLK item.

**FLK3 -- `d2=false`. The lane asked whether the loop's exit condition was wrong
or the request never arrived. It is both, and they are two separate bugs.**

- *The request arrived and was discarded.* The proof-wait loop matched only
  `PacketType::Proof` and sent everything else to `Ok(Ok(_)) => continue`. RNS
  opens its direction-2 link as soon as it sees the announce, which can easily
  precede its proof of ours -- so the inbound `LinkRequest` hit that discard
  branch, and direction 2 then had no link to arrive on. Fixed by accepting an
  inbound link concurrently during the proof wait.
- *The request was never sent, because RNS never saw the announce.* The gate
  registers its destination **before** `RNS.Transport.register_announce_handler`,
  leaving a window in which a link can be proved but an announce is processed
  with nothing listening. The retry loop stopped at the proof, so the announce
  was never resent. Signature: `dir1=PASS`, `rns_saw_svc` absent, `d2=false`.
  Fixed by re-announcing every second until the responder link exists.

**Census progression on `interop_reqresp`, n=120 each: 7 failures in 4 modes ->
3 in 2 -> 1 in 1.** Read that as three modes going to zero alongside three
identified mechanisms, **not** as a rate measurement. The rate is still FLK1's
job at its declared n, and this document's arithmetic on that point stands
unchanged.

## What is not established

The connection-establishment mode above survives at roughly 1 in 120, and the
750 ms settle that reduced it is an uncharacterised guess.

**Every rate in this document was measured on a heavily contended machine, and
none of them should be compared against a rate measured elsewhere.** During the
2026-08-23 work the box was running 54 rustc and 14 cargo processes across 16
logical cores -- a 3.4x oversubscription, varying continuously as other sessions
started and finished builds. Each live gate is timing-sensitive localhost
networking, so under that load the measurements describe the machine at least as
much as the gate. This plausibly accounts for several of the failed attributions
recorded above: a blocked 2x2 that appeared to show a 20% RNS 1.5.0 regression
and vanished when interleaved, a failure that disappeared at log level 7, and a
rebuild hypothesis that would not reproduce. Load drift is a single explanation
for all three.

The consequence for FLK1 and FLK2 is direct: **a declared n is not sufficient if
the load is uncontrolled.** A 175-run baseline taken during a build storm and one
taken on a quiet machine are not the same measurement and cannot be pooled.
`flake_census.py` now records concurrent rustc and cargo counts at the start and
end of every census for exactly this reason, and discards runs that died in the
build rather than in the gate -- a shared target directory under this much
parallelism manufactures stale-rlib failures that are not gate failures at all.
The three censuses reported above were checked for that contamination and had
none, but they were not protected against it at the time.

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
3. **FLK3 — explain `d2=false`. DONE**, see the amendment above: two bugs, a
   discarded inbound `LinkRequest` and an announce lost to the gate's
   handler-registration window. Both fixed; the mode is at zero in 120 runs.
4. **FLK4 — explain the whole-exchange collapse. EXPLAINED, NOT CLOSED.** It is
   connection establishment, as this item guessed. The mitigation is a raised
   settle wait rather than a fix, and the mode survives at about 1 in 120.
   Characterising how long RNS actually needs before its first frame — instead
   of guessing 750 ms — is the open remainder and should carry its own number.
5. **FLK5 — set the evidence policy.** Once per-gate rates are bounded, state in
   the oracle README what a passing suite run is allowed to prove, and stop
   quoting bare "twelve of twelve" counts in receipts without a rate alongside.

FLK1 and FLK2 are mechanical and can run unattended, and should use the census
tool rather than bare pass counts. FLK3 is done and FLK4 is explained; neither
now blocks on FLK1.

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
