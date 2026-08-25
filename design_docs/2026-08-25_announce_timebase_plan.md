# Announce timebase plan

**Date:** 2026-08-25
**Status:** open plan. Nothing in it is implemented yet.
**Owns:** the announce `rand_hash` field, the receive-side path-acceptance rule, the
firmware tier's monotonic timebase, and the clean-room rule for microReticulum.

**Related authority:** [wire format reference](2026-07-13_rns_wire_format_reference.md)
(needs the corrections in §6 below), [RNS 1.5.0 re-pin
receipt](2026-08-23_rns_150_lxmf_111_repin_receipt.md), [live-gate flake
lane](2026-08-23_live_gate_flake_lane.md), [Prns harvest
brief](2026-08-09_prns_harvest_brief.md).

---

## 1. What is settled, and how

Two surveys on 2026-08-24 and 2026-08-25 read fourteen independent Reticulum
implementations at source, licence verified by opening the file in every case. The
findings below are not inferences from the manual; each is read in source or observed in
bytes.

**The 10-byte announce field at payload offset 74..84 is not opaque.** It splits: bytes
0..5 a per-emission random nonce, bytes 5..10 a 40-bit **big-endian count of whole
seconds**.

Confirmed three independent ways, none of which required reading the Python reference:

1. **Stock RNS's own emitted bytes.** Committed captures decode to their own capture
   wall clock: `1786506815` is 2026-08-12 03:53:35 against directory
   `peer-20260812T035333Z`, and `1787537807` is 2026-08-24 02:16:47 against
   `peer-20260824T021646Z`. The field delta of 1,030,992 s is 11.93 days, matching the
   directory gap exactly.
2. **Fixtures already in this tree.** `announce_plain.bin` at offset 93 gives
   `f2d091f887 006a55a78a`, and `0x006A55A78A` is 1,783,998,346, which is
   2026-07-14T03:05Z, its capture date. `announce_ratchet_appdata.bin` is `006a55a78b`,
   exactly one second later.
3. **RNS's own persisted routing state.** The msgpack `destination_table` RNS writes in
   every gate run is
   `[dest(16), timestamp f64, received_from(16), hops u8, expires f64, random_blobs
   [bin10 ...], iface_hash(32), packet_hash(32)]`. `random_blobs` is a **list**, which is
   RNS's own output confirming it retains this field per destination for comparison.

**The field is a monotonic counter, not a clock.** Eight implementations that implement
announce acceptance were read. **Not one performs a calendar check, a skew check, a
plausibility window, or any comparison against the local clock.** Every one compares
ordinally, per destination, against blobs stored for that destination. Three of them have
a real wall clock available and still decline to use the field as one. Prns names the
type `MonotonicTimebase` rather than `UnixTimestamp` for this reason.

**Consequence: any monotonically increasing 40-bit counter interoperates.** A real epoch
second is not required by any receiver surveyed. This is the load-bearing assumption of
the firmware work in §4, and probe P3 exists to test it rather than trust it.

**The asymmetry that makes the fix direction matter.** Monotonicity is enforced per
destination against that destination's own prior emissions. There is no cross-node
timestamp comparison anywhere in the protocol.

- **Starting low is harmless.** microReticulum emits values in the thousands and it never
  bites.
- **Starting high is poison.** Once a value above what the node can subsequently count up
  to is latched, the node can never beat its own high-water mark again.

**The acceptance boundary is strictly-greater, on a seven-to-one tally**, with the one
dissenter demonstrably broken in the same function. See §5.

---

## 2. Defects, ranked

### D1. All ten bytes are CSPRNG. Deployment-affecting.

Three emission sites:

| site | what it emits |
| --- | --- |
| `crates/retinue/src/endpoint.rs:4568` `rand_hash()` | 10 bytes `getrandom`, via `Endpoint::announce` (2973) and `path_response` (2144) |
| `crates/radio-hand/src/channel/node.rs:501` | 10 bytes `exec.random`, fresh every Beat |
| `crates/retinue/examples/*` (about 10 files) | low-order LE nanoseconds, **worse than random** |

`crates/retinue/src/announce.rs:189` documents the field as "a fresh random one per
announce", which is the wrong contract and is where the defect is written down.

**Observed in our own artefacts.** Announces Python RNS cached during retinue's own
peering runs decode as:

```
peer-20260812T035508Z:  0c713c0192 f2ca180000  ->  1,042,772,656,128
peer-20260824T021646Z:  dcaecd302a 9cce180000  ->    673,450,065,920
```

Retinue's advertised emission time **regressed by 369 billion over twelve days**.

**Severity, stated accurately.** An earlier framing of this as "poisons a peer for seven
days" overstated it. Because retinue draws fresh randomness per announce, the Nth is
accepted with probability about 1/N, a running maximum of uniform draws, so roughly ln(N)
of N announces land. First contact always succeeds and route expiry heals. It **degrades
rather than breaks**. The two real harms are sharper than the framing suggested:

- A genuinely better, lower-hop path is discarded most of the time by the `hops <=
  existing` gate, so route optimisation toward a retinue node is broken.
- Where a rejected announce also suppresses onward retransmission, announces stop
  propagating past the first hop.

The nanosecond variant in the examples is worse than random rather than better: taking
the little-endian low ten bytes and having them read back big-endian over bytes 5..10 puts
the fastest-moving byte in the most significant decoded position. Measured on committed
captures, it plateaus for 18.3 minutes and regresses roughly every 78 hours.

### D2. No monotonic timebase exists on the firmware tier at all. Deployment-affecting.

`SystemTime` and `UNIX_EPOCH` appear nowhere in `crates/retinue/src`,
`crates/radio-hand/src` or `crates/outrider/src`.
`crates/retinue/src/announce_admission.rs:118` states the discipline: "Times are monotonic
milliseconds relative to the endpoint's creation." That resets to zero on every restart.

**Even with D1 fixed, a firmware node would regress on every reboot** without a persisted
high-water mark. This is not a consequence of D1; it is a separate missing capability.

### D3. Path table has no emission-time gate and no replay set. Deployment-affecting.

`endpoint.rs:2164`: `keep_existing = e.hops <= hops && now.duration_since(e.learned) <
PATH_TTL`. `node.rs:657`: `if route.hops <= hops { return }`. Hop count plus retinue's own
local clock. Nothing in `crates/` parses, compares or remembers `rand_hash`; its only
consumer workspace-wide is a test.

This matters because **a transport node answering a path request replays the original
announce payload verbatim**, same nonce, same embedded timestamp, rewriting only hop count
and header. So an ancient announce arriving at one fewer hop unconditionally replaces a
live route and is held for up to `PATH_TTL`. This is not adversarial; it is routine
transport behaviour that `path::path_request` actively solicits.

**The near-miss is the useful part.** Retinue's packet hash masks out hops, header type
and transport ID (`packet.rs:224`), so a re-stamped replay hashes identically to the
original and `announce_is_new` (`endpoint.rs:2265`, a 4096-entry window) would catch it.
But that dedup is consulted at `endpoint.rs:3750` for the **relay** decision only, and
`learn_path` already ran at `endpoint.rs:3735`. The mechanism exists and is applied
fifteen lines too late.

### D4. Equal-hop announces never update the route. Minor.

`endpoint.rs:2164` and `node.rs:657` both keep the incumbent on `hops <= hops`. RNS
accepts an equal-hop announce when strictly newer, so a peer that moves to a different
equal-length path is not followed until `PATH_TTL` expires.

---

## 3. The emit fix

Shape is settled; three decisions inside it are the maintainer's and are flagged, not
taken.

- Bytes 0..5 from the CSPRNG, unchanged.
- Bytes 5..10 a big-endian whole-second count from a monotonic source.
- **Quantise to seconds before truncating.** 40 bits of seconds is about 34,865 years; 40
  bits of microseconds is 12.7 days. reticulum-zig truncates a microsecond counter and
  wraps in under a fortnight. One line, fatal if wrong.
- **Stay under 2^32 while in uptime mode.** Naturally true for a boot-relative counter. It
  matters because at least one Go port reads the 40-bit field into a `uint32` and silently
  discards the top byte, so a value with bits 32..39 set compares differently on that peer
  than on a Python one. Avoid triggering it; do not rely on it.

**Decision M1: does `announce::build` keep taking an opaque `[u8; 10]`?**
The sans-io contract is correct and not in question: retinue has a `no_std` firmware tier
and a tokio host tier, one implementation serves both, and a core that reached for a clock
or an RNG could not compile for the boards. It also buys byte-exact fixture pinning.
Sans-io says the caller supplies entropy and time; it says nothing about the parameter
being structureless. The choice is only whether the type carries the split so callers
cannot get it wrong. Four callers currently get it wrong. Prns's answer is a move-only
entropy type consumed at mint, which is parse-don't-validate applied and is still fully
sans-io.

**Decision M2: what the `no_std` tier puts there.** See §4.

**Decision M3: remediation.** No remediation appears to be owed. Every peer that has heard
retinue is a throwaway RNS instance in a temp directory: 45 of 54 oracle scripts use
`tempfile.mkdtemp` and the rest delegate to those that do, no `destination_table` exists
outside per-run capture directories, and retinue has never been pointed at a public
Reticulum node or a testnet. The poisoning is real and demonstrated but did not survive
its test run. **The bill comes due the first time retinue announces to something
persistent.** Fixing before that costs nothing.

---

## 4. The firmware timebase

### What anyone actually implements

| strategy | implemented by | status |
| --- | --- | --- |
| persisted high-water mark in flash | Prns | shipped, dual-region, tested against interrupted writes |
| persisted cumulative-uptime counter | microReticulum | shipped on ESP32, three defects below |
| raw monotonic uptime, no persistence | reticulum-zig | shipped, wraps in 12.7 days, regresses every boot |
| refuse to announce until firmware supplies time | LXMF-rs | contract shipped, **seam never wired** |
| acquire from a heard announce | nobody | zero of eleven |
| NTP, GPS, or an RTC driver | nobody | zero of eleven, grep-confirmed |
| time from a host or peer handshake | nobody | zero of eleven |

The design instinct that the announce timestamp should ride whatever time-acquisition
story the node already has, rather than being special-cased, is **unprecedented in the
surveyed corpus**. LXMF-rs is the only implementation that even provides the seam
(`RnsError::TimeSourceUnavailable`, refusing to announce until `set_time_override` is
called), and nothing in its embedded crates, runtime or C ABI ever calls it.

### What retinue has to work with

Both boards have RTC peripherals. `firmware/t114-phy` runs `embassy-nrf` with
`time-driver-rtc1`; `firmware/heltec-v4-phy` has `esp_hal` TimerGroup and hands the RTC to
`power::arm`. What they lack is not a clock but **epoch knowledge at boot**, which per §1
the wire does not require.

### Recommendation

Synthesised from what these implementations do and where they broke. Each is a decision to
be taken, not one taken here.

1. **Whole seconds, never sub-second.** As §3.
2. **A persisted high-water mark in flash, u64 internally.** Prns's shape, not
   microReticulum's. Never apply a magnitude heuristic to the persisted value:
   microReticulum discards any offset above `4294967295`, which is a ceiling of 49.71 days
   of cumulative uptime, reachable without a reboot, after which the timebase collapses to
   zero permanently.
3. **On restore, round up to the next whole second and bump by one whole second.**
   microReticulum bumps by one **millisecond** against a **whole-second** wire field, and
   persists every 600 s, so its post-crash announce blackout is up to ten minutes.
   Round-up-on-restore collapses that to zero regardless of persist interval. **The
   persist interval is otherwise the blackout window**, which makes it a correctness
   constraint rather than a tuning knob.
4. **Tag the representation.** A bare integer cannot distinguish "uptime since first boot"
   from "epoch-derived". Once a board acquires a real epoch the counter jumps forward,
   which is monotone and fine; the reverse is not, and a tagged enum makes the illegal
   transition unrepresentable. microReticulum's single untagged offset is incoherent for
   exactly this reason: its u32 ceiling silently forecloses the epoch regime.
5. **Adopting a real epoch is a separate, later decision.** The announce wire does not need
   it. The things that do want one, based on what the surveyed implementations gate on wall
   time, are ratchet expiry (30 days), link-request freshness, and cross-node log
   correlation. None of them is this field.

---

## 5. The receive fix

**Tally at equal-or-better hop count, across both surveys:**

| strictly greater | accepts equal | no comparison |
| --- | --- | --- |
| microReticulum, reticulum-kt, reticulum-swift, Quad4 Go, go-reticulum, LXMF-rs, Prns | reticulum-zig | ReticulumKit, rns.js, one Go fork |

Seven to one, and the one is broken in the same function: its stricter term is a subset of
its looser term, so the hop comparison is dead code and hop count plays no role in
acceptance at all. Read it as incomplete, not as a dissenting vote.

**Two field post-mortems corroborate independently of any source reading.** reticulum-kt
commit `3e22e7e` *added* the emission-time gate after a deployed network showed phones
holding stale path-response entries at four hops that fresh one-hop announces could not
overwrite. reticulum-swift carries a comment from someone who hit the mirror failure:
without a proper timestamp "the relay's deduplication logic will reject our announces".

**Both conditions are required.** The comparison target is `max()` over a stored **list**
(64 blobs in most implementations; 32 in memory with 16 persisted in microReticulum), not
a scalar. A scalar-only implementation accepts a replayed blob whose timestamp equals the
max; a list-only implementation accepts a stale one.

**Recommendation:** implement strictly-greater plus blob-novelty at `hops <= existing`, and
a separate looser branch at `hops >` (expired implies blob-novelty alone, else strictly
greater). **Defer both equality carve-outs** (the unresponsive-path repair, and the RNS
1.4.1 interface-gravity tiebreak) until measured. They only ever add acceptances, so
omitting them is the conservative error.

Fixing D3 also means moving the `announce_is_new` consultation to before `learn_path`, or
giving `learn_path` its own replay check. The window already exists.

---

## 6. Wire reference corrections

Against [the wire format reference](2026-07-13_rns_wire_format_reference.md).

1. **Close O-20** and rewrite §3.3.4. The 10 bytes are 5 random plus 5 big-endian whole
   seconds. Confirmable from fixtures already in the tree, per §1.
2. **Raise O-20's impact from "Low".** The current rationale, that the field is opaque to a
   verifier and only affects dedup and freshness, is wrong: it is opaque to a *verifier* but
   decisive to a *transport node's path table*, and it gates onward retransmission. It is
   the difference between being routable and not.
3. **Split the two payload tables** (the field table and the offset table) into nonce(5) and
   timestamp(5, BE seconds) rows, sourced to the captures rather than to Beechat. Note that
   Beechat's `SHA256(...)[0..10]` line describes its **pre-`d4fc67d`** behaviour and is now
   historical: that commit, "fix: announces include timestamp in random blob", is an
   independent author discovering this defect by interop testing.
4. **Expand §3.3.5** from a note that retinue has no dedup or freshness check into the named
   gap, with the rule from §5 written out.
5. **"Expires routes at exactly 7.0 days" is the default-mode figure only.** Six independent
   confirmations give per-interface expiry: ACCESS_POINT 24 h, ROAMING 6 h, else 7 days.
   Retinue's `no_std` nodes live on roaming links, so their blast radius is six hours.
6. **Add `PATHFINDER_M = 128` with its asymmetry:** admit at `hops <= 128`, retransmit only
   at `hops < 128`. Invisible from the manual.
7. **Add: path-response announces are the same payload bytes with only the context byte
   changed**, and forwarders must never regenerate the blob. Multi-path hop comparison
   depends on identical blobs arriving over several routes.
8. **Add `LOCAL_REBROADCASTS_MAX = 2`**, suppression keyed on `packet.hops - 1 ==
   entry.hops`.
9. **Add the RNS destination-table format** from §1, as retinue's first direct observation
   of RNS's routing state.
10. **Add an ecosystem-hazard note, not a spec fact:** at least one Go port truncates the
    40-bit timebase to `uint32`.

---

## 7. Probes

Black-box observation of stock RNS output is always available and is how the field layout
was confirmed. It is the default instrument.

| # | question | instrument | gates |
| --- | --- | --- | --- |
| **P1** | Does stock RNS accept an equal-timestamp announce at equal hops? | Two announces from one destination in the same wall-clock second, different nonces, against a **persistent** RNS config. Read `destination_table` between each; `random_blobs` grows only on acceptance. | §5. Seven implementations predict rejection and **nobody has measured it.** |
| **P2** | Does a poisoned high-water mark actually lock a corrected retinue out? | Announce once with an absurdly high timestamp half, then with a correct one; check whether `destination_table` grew a second blob. | the severity claim in D1 |
| **P3** | Is a received timebase range-checked anywhere? | From one destination, emit timebase `1` (1970) and `2^39`; read `destination_table`. | **§4 entirely.** If a boot-relative counter is rejected, the whole firmware recommendation collapses. |
| **P4** | How many blobs does RNS retain? | 70 announces at 1 Hz into a persistent instance, count the array. | the list-versus-scalar half of §5 |
| **P5** | Does stock RNS gate link requests on a wall-clock `requested_at`? | Capture a stock LINKREQUEST payload, look for a msgpack timestamp. | whether a clockless board can do anything beyond announcing |
| **P6** | What does stock RNS do with bits 32..39 set? | Same instrument as P3 with `2^33`. | whether the Go truncation is a lone bug |
| **P7** | Path-request addressing: to the target hash, or to a well-known `rnstransport.path.request` destination? | Capture what `rnpath <hash>` emits. | one implementation disagrees with our reading |

P1 and P3 are the two that gate real decisions. P3 gates more.

**Every probe needs a persistent RNS config directory.** Every current gate uses
`tempfile.mkdtemp`, which is why all nine committed runs contain exactly one entry with
exactly one blob: **the suite only ever exercises the first-sighting arm, which accepts
unconditionally.** It would pass with the field set to all zeroes. That blindness is
structural and should be recorded in the validation section of the wire reference.

---

## 8. Clean-room rule: microReticulum comment blocks

**microReticulum's Apache-2.0 sources contain large volumes of verbatim Reticulum Python
reference source in comments**: 23 `/*p ... */` blocks, 153 `//p` lines, 41 `//z` lines,
with no NOTICE file and no attribution to the Python author anywhere.

**Standing rule: treat `/*p`, `//p` and `//z` in microReticulum as black-box, exactly like
the Python reference itself.** The surrounding C++ is Apache-2.0 and readable; those
comment blocks are not, whatever the repository's own licence file says, because the
licence a repository declares cannot relicense someone else's code that it has pasted in.

One surveyor read two such blocks before recognising the pattern and disclosed it. Nothing
was carried into a recommendation and no retinue code is affected: retinue's IFAC came from
captured bytes and published documentation.

This also makes microReticulum a **weaker reference than its prominence suggests**. Where
it is most complete, much of what one would learn from it is upstream Python in disguise.

This rule belongs in `crates/retinue/oracle/README.md` beside the existing black-box
discipline, and in `crates/outrider/PROVENANCE.md`.

---

## 9. Done-conditions

- No emission site in the workspace writes randomness into bytes 5..10. Verified by a test
  that decodes an emitted announce and asserts the timestamp half advances between two
  emissions more than one second apart.
- A firmware node's timebase survives a reboot without regressing. Verified on hardware,
  not in a host test.
- The path table rejects an announce whose timestamp does not exceed the stored maximum for
  that destination at `hops <= existing`, and rejects a replayed blob at any hop count.
- A gate exists that exercises the non-first-sighting arm, meaning at least one gate uses a
  persistent RNS config across two announces.
- P1 and P3 are answered, or the plan records why the conservative subset was adopted
  without them.
- The wire reference carries the §6 corrections and O-20 is closed.
- The clean-room rule in §8 is recorded in both provenance documents.

## 10. Explicitly out of scope

- Adopting a real epoch anywhere. Separate decision, §4.5.
- The equality carve-outs in §5. Deferred pending measurement.
- Ratchet expiry, link-request freshness, and anything else that wants a wall clock.
- The live-gate flake, which has [its own lane](2026-08-23_live_gate_flake_lane.md).
- Any change to Prns, microReticulum, or any other surveyed implementation. We read them;
  we do not carry patches to them.
