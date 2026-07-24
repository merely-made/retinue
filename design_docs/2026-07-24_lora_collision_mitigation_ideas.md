# LoRa collision mitigation: ideas worth keeping

**Status (2026-07-24): notes, not a plan.** Nothing here is scheduled, and most
of it is not reachable from the hardware this project ships on. Recorded so the
reasoning survives instead of being re-derived.

**Provenance.** These are techniques read from public documentation. There is no
maintained implementation to consult and none was consulted; the licensing on
what does exist is unfavourable, so this stays at the level of physics and
method. Techniques are not copyrightable, only expression is, so implementing
any of this independently from public description is clean — the same discipline
`crates/sennet/PROVENANCE.md` already runs on.

## The idea underneath all of them

Each technique converts a **persistent, packet-destroying collision** into
**sparse, random erasures**, and erasures are what forward error correction is
good at. That ordering is the contribution: make collisions rare and *scattered*
first, then make the remainder survivable. Four parts, in the order they depend
on each other:

1. **Deliberate frequency error.** LoRa demodulation tolerates carrier offset up
   to roughly a quarter of the modulated bandwidth, because offset appears as a
   cyclic shift in the dechirped FFT rather than as lost signal. Detuning
   concurrent transmitters differently puts their peaks in *different bins*, so a
   receiver can lock one instead of resolving a smear. This is the clever part:
   it rescues the case the capture effect cannot, where two signals arrive at
   near-equal power and neither is ~6 dB up on the other.
2. **Sub-symbol timing offsets and per-symbol frequency hopping**, so two
   concurrent floods collide only on the symbols where their hop sequences happen
   to coincide.
3. **Slotted ALOHA on the preamble.** The preamble cannot hop — the receiver has
   to find it somewhere known — so it is the one window the hopping scheme
   cannot protect, and it gets its own mechanism.
4. **Stronger FEC** (convolutional/Viterbi or Reed-Solomon) over LoRa's built-in
   Hamming, to clear the residue.

Point 3 is the tell that this is honest engineering rather than a pitch: the
authors named the hole their own scheme could not cover instead of omitting it.

## Costs the description does not carry

- **The frequency budget is not free.** The same tolerance absorbs crystal
  error, thermal drift, and doppler. At 915 MHz a 10 ppm part is already ~9 kHz,
  which is ~7% of a 125 kHz channel before anything is injected deliberately. A
  cold or drifting node can walk off the edge of the tolerance. It wants per-node
  calibration, and it costs some sensitivity, since offset degrades the dechirp.
- **Frequency and timing offset are coupled in LoRa** (the CFO/STO ambiguity):
  injecting one perturbs the apparent other. Techniques 1 and 2 are therefore not
  independent knobs but movements in one 2D space, which is probably why they
  appear together.
- **Slot synchronisation has a system cost.** Slotted ALOHA needs a shared clock,
  which without GPS means beacons, energy, and drift management. It sits
  particularly badly with the low-power plan, where the ESP32-S3's time source
  does not advance in Light-sleep — a node cannot both sleep through the quiet
  and keep slot discipline without a separate clock-compensation design.
- **Custom FEC costs compute and doubles overhead** unless LoRa's own coding is
  turned down, and Viterbi decoding on a sleeping node is not free.

**One upgrade the description does not mention:** hopping means the receiver
*knows which symbols were hit*. Feeding those positions to a Reed-Solomon decoder
as **erasures** rather than errors roughly doubles the correction power for the
same parity. If an implementation is not doing that, it is the cheapest available
win.

## Whether any of it is reachable here

The load-bearing question is per-symbol hopping. The SX127x family exposes a
hop-period register in LoRa mode, so symbol-granular hopping is a real chip
feature there. **Whether the SX1262 offers an equivalent for standard LoRa is
unverified** — LR-FHSS on SX126x is a separate modulation and transmit-only on
those parts, with reception needing gateway-class silicon. If the scheme needs an
SX127x or an SDR, it does not run on the V4s or the T114, and that single fact
settles its relevance to this project. Verify before spending anything on it.

Regulatory: deliberate hopping moves a device under FCC 15.247's frequency-hopping
rules (channel count, dwell time) rather than the digital-modulation rules. That
could simplify or complicate certification, and it interacts with the
region-locked-firmware posture in `2026-07-20_fcc_reselling_flashed_radios.md`.

## Which layer this is, and why it is mostly not ours

This stack is collision **survival**, at the PHY. What this project has built is
collision **avoidance**, at the MAC: Tulle's airtime budget keeps the node off
the air when it should not be, and the V8 transit scheduler decides who goes
first when the queue is deep. They are complementary, and the MAC answer is the
one that works on stock certified radios today. Adopting the PHY stack means
owning the modem, which is a different undertaking from managed Reticulum nodes.

There is also a protocol-shape reason the medicine is aimed elsewhere. This is a
cure for **flooding** meshes, where many neighbours rebroadcast the same packet
at once. Meshtastic's managed flooding has that disease acutely, so **Sennet
inherits it**. Reticulum is addressed and path-routed: retinue floods announces,
not data, so the exposure is real but far narrower.

## The part worth taking now

The *slotting* insight, without any of the PHY: when several neighbours relay the
same announce, they currently do it as fast as the router hands it over, which
means they collide with each other. **Randomised jitter before relaying an
announce** spreads them across time, needs no PHY control, changes nothing on the
wire, and removes the most likely self-collision in a retinue mesh. That is the
5% of this stack that fits the architecture, and it belongs next to
`broadcast_transit` in `crates/retinue/src/endpoint.rs`.
