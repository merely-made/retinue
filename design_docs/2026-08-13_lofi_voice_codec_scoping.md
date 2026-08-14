# Lofi voice codec scoping

**Date:** 2026-08-13
**Status:** Scoping note. Names **Pipit** as the "cheap cheap" speech-codec
sibling of wavicle's hifi lane, first for the store-and-forward voice drops in
the [IoT device concepts note](2026-08-13_iot_device_concepts.md), with later
live voice calls kept in scope.

**Rungs 0 and 1 landed 2026-08-13.** `repos/pipit` is founded (MIT/Apache,
ed2024, zero dependencies, `no_std`, `forbid(unsafe_code)`) with framed IMA
ADPCM, the 2,489 bps LPC vocoder, and the clip container. 38 tests plus a
doctest green; clippy clean; compiles for `thumbv7em-none-eabihf`
(nRF52840/T114) and `riscv32imac-unknown-none-elf` as well as the host.
Published as `pipit` 0.1.0 and public at
[merely-made/pipit](https://github.com/merely-made/pipit). Carriage into
outrider landed the same day; see the seam section below.

Rung 0 measured 34.8 dB steady-state SNR, converging within 10 ms from a cold
start, with an isolated frame decoding within 0.5 dB of the same frame in a
continuous stream. Rung 1 measured 1.97 dB mean log spectral distortion with
pitch recovered within one quantizer step. See each rung below for what was
decided along the way.

## The requirement

The first consumer is not realtime: Outrider propagation delivers a recorded
memo over minutes or hours, so its codec bitrate matters through payload size
and airtime rather than latency. The same framed codec should remain usable for
later live or push-to-talk calls over capable LoRa paths; call control, jitter,
loss recovery, and regional airtime limits belong outside the codec. Targets:

- 8 kHz mono speech, intelligibility over fidelity. Lofi is the identity, not
  a compromise: wavicle is the hifi lane, this is deliberately the other end.
- 1 to 3.2 kbps over LoRa. A 10-second memo at 2.4 kbps is 3 KB, roughly six
  Reticulum-MTU frames, comfortably inside R4 resource carriage and duty
  cycle. At 32 kbps the same memo is 40 KB, which is a WiFi-bearer payload,
  not a LoRa one.
- Encode and decode both fit the boards: single-precision FPU (nRF52840,
  ESP32-S3), no_std, alloc-free, table-driven.

## What a Codec2-class codec looks like inside

For scoping honesty, the pipeline every codec in this class implements:

1. Frame 10 to 40 ms of 8 kHz samples.
2. Estimate pitch (Codec2 uses the NLP estimator).
3. Model the frame as harmonics of the pitch: extract per-harmonic
   magnitudes from a DFT.
4. Decide voicing (voiced/unvoiced, per frame or per band).
5. Quantize pitch, energy, and the spectral envelope (LPC-to-LSP scalar
   quantization in the mid modes; vector quantization of mel-spaced
   magnitudes in the lowest).
6. Pack to a fixed frame: Codec2's 3200 mode is 64 bits per 20 ms; 1300 is
   52 bits per 40 ms.
7. Decode by resynthesizing the harmonic bank with synthetic phase, then
   postfilter.

All f32 DSP over fixed tables, no allocation. That is why this class fits
microcontrollers at all.

## Landscape (verified 2026-08-13)

- **`codec2` crate** (crates.io): pure-Rust translation of the C reference,
  modes 3200 down to 1200, active (0.3.1, July 2026), sole dependency is
  micromath, which signals embedded-friendly design. License **LGPL-2.1 AND
  MIT**, and that is the problem: retinue's `deny.toml` makes LGPL a hard red
  line for the dependency tree. The crate cannot ship in the workspace.
  It remains valuable **outside** the workspace as a black-box oracle and
  quality comparator, the exact posture of the RNS and Prns checkouts.
- **C codec2** (David Rowe): LGPL-2.1, the reference. No formal bitstream
  specification exists; the implementation is the spec, though Rowe's papers,
  thesis, and in-tree documentation describe the algorithms extensively.
- **LPC-10e**: the 2.4 kbps US federal standard (FIPS-137), 10th-order LPC
  plus pitch, voicing, and energy, 54 bits per 22.5 ms frame. A published
  public specification, decades out of any patent, and the simplest real
  vocoder in the field. Quality is the classic robotic lofi voice.
- **ADPCM (IMA)**: trivial (roughly 100 LOC from the public spec), 32 kbps at
  8 kHz. Wrong for LoRa, right for WiFi-radius drops and for proving the
  pipeline.
- **Opus**: bitrate floor around 6 kbps, no mature pure-Rust encoder, C
  binding heavy for MCU. Out.
- **MELPe**: patent-encumbered. Out.
- **Neural codecs**: out of MCU scope entirely.

## Direction: three rungs

**Rung 0, prove the pipeline: ADPCM. DONE 2026-08-13.** Own-implemented from
the public description, permissively licensed. Voice drops over WiFi bearers
(house radius, and any TCP-bearer path) work immediately, and the drop format
is exercised end to end before any vocoder exists. ADPCM stays useful
permanently as the WiFi-tier codec.

Two findings worth carrying into Rung 1, both measured rather than assumed:

- **Cold start is the transient that matters.** IMA's step index begins at 0,
  so a coder starting cold sits near 8 dB for its first 5 ms and reaches full
  quality by 10 ms. It is quiet-then-correct, not wrong, but a codec that
  restarts state per message pays it on every message. LPC-10e has its own
  version of this in gain and pitch tracking; measure it the same way.
- **Carrying coder state in the frame header costs 3 bytes and buys loss
  independence.** An isolated frame measured within 0.5 dB of the same frame
  decoded in a continuous stream. That is what makes the same codec usable
  for calls, and Rung 1 should keep the property rather than rediscover it.

**Rung 1, the owned lofi vocoder: LPC-10. DONE 2026-08-13.** The "cheap
cheap" codec. Own-licensed, no clean-room anxiety, no deny.toml conflict.
The robotic timbre is on-brand for a radio product; lofi is the point.

Landed as `pipit::lpc10`: 10th-order, 22.5 ms frames, 7 bytes per frame
(2,489 bps), 13x smaller than Rung 0. Ten seconds of speech is 3.1 KB
against ADPCM's 40 KB. Measured 1.97 dB mean log spectral distortion, pitch
recovered within one quantizer step from 80 to 400 Hz, and voiced, unvoiced,
and silent frames classified correctly. About 750 lines of `no_std` Rust
including its own float math, which came in at the low end of the estimate.

**Conformance, decided during the build and worth recording plainly.** The
plan said "faithful to FIPS-137", and the published *structure* was
available: 180-sample frames, order 10, and the bit allocation (5 gain, 7
pitch and voicing, then 5/5/5/5/4/4/4/4/3/2 across the reflection
coefficients, plus a sync bit). The standard's *quantizer tables* were not
reachable from any source at hand. Inventing numbers and calling them
FIPS-137 would have been the worst outcome, so the implementation follows the
structure exactly and uses its own tables, and says so everywhere it could
mislead.

The cost of that is bitstream interoperability with other LPC-10e
implementations, which was weighed and found to be worth almost nothing here:
there is no live LPC-10e ecosystem to talk to, and Rung 2 was always the
interop rung (FreeDV, M17). What Rung 1 was actually chosen for, a
license-clean vocoder small enough for LoRa, is delivered. If conformance is
ever wanted it becomes a new codec identifier in the clip header rather than
a format break, which is what that field is for.

**Rung 2, later and optional: a Codec2-class coder.** Better quality per bit,
and bitstream compatibility with Codec2 would buy interop with the existing
amateur ecosystem (FreeDV, M17 uses Codec2 modes). But there is no formal
spec, so compatibility means either an LGPL-derived translation (dead inside
the workspace by deny.toml; possible only as a forever-external component) or
an independent implementation from Rowe's publications verified black-box
against the reference, which is a genuinely large DSP project. **That decision doc now exists and closes this rung as
framed: see
[Rung 2 decision](2026-08-13_rung2_codec2_class_decision.md).** In short,
Codec2's low modes quantize against trained codebooks shipped only in the
LGPL tree, and a decoder cannot read the bitstream without the exact
codebook the encoder searched, so an implementation from publications cannot
reach compatibility at all. FFmpeg, which prefers native decoders, still
requires libcodec2 for the same reason. Interop, if wanted, is an
application-tier project linking the real library under GPL, where retinue's
GPLv3 firmware images already make that legal. What replaces the rung is a
half-rate superframe mode: 1,600 bps against the present 2,489, no trained
data required.

## Seam

The drop format carries a codec identifier and mode alongside the payload, so
rungs coexist: ADPCM drops on fat bearers, LPC-10e drops on LoRa, and a future
codec slots in without a format break. **Pipit** itself is a
transport-agnostic standalone sibling crate in wavicle's posture
(crates.io-only deps, consumable by firmware and host alike), not a retinue
workspace member, so host apps outside the radio family can decode drops and
calls too.

**Refinement made while building Rung 0 (2026-08-13).** This note originally
put the whole drop format on the outrider/retinue side. Implementation split
it in two, because the original placement contradicted the sentence above it:
if parsing a drop requires an MPL radio crate, a host app outside the radio
family cannot decode one.

- **The clip is pipit's.** A clip is a self-describing payload: magic,
  version, codec id, mode, sample rate, frame geometry, sample count, then
  frames. It is the half that must be readable without a radio stack, and it
  is where the codec-identifier requirement above is actually satisfied. Rung
  1 exercised it as designed: adding the vocoder was a new codec id (2) and a
  frame-geometry constraint, with no change to the container.
- **The envelope stays retinue's.** Sender, recipient, timestamp, signature,
  and routing never enter a clip. Outrider or a resource transfer wraps it.

### Carriage landed 2026-08-13, and the field-number question answered

`outrider::voice`, behind an opt-in `voice` feature, attaches a clip to an
LXMF message and takes it back out. A boundary crate should not carry a speech
codec for consumers who only send text, hence the feature; with it on, outrider
takes a `pipit` dependency (MIT/Apache into MPL, no dependencies of its own,
and the license gate passes with it). Ten unit tests, including an end-to-end
one: attach, sign, encode, decode, recover the clip byte for byte, and decode
it back to audio that still has energy in it.

**The open question above was which LXMF field carries a clip. It turns out
not to be answerable from public prose, which changes the answer rather than
delaying it.** LXMF's README states that full protocol documentation is still
planned, and the audio field's mode list is published only as source, which
outrider does not read. The v1 scope in the
[outrider founding doc](2026-07-25_outrider_lxmf_founding.md) already rules
that fields enter "from captures and public prose" only, so there is no
legitimate route to that number today short of a black-box capture against the
pinned LXMF 0.9.6 oracle.

**Both points below were superseded on the same day by an actual capture:
see the [LXMF field registry capture](2026-08-13_lxmf_field_registry_capture.md).
Audio is field 7, and the mode enumeration is not closed: `AM_CUSTOM` means
"a codec outside this list", so a clip rides the stock audio field honestly
after all. Point 2 was careful reasoning about a registry nobody had looked
at, and it reached the wrong conclusion.** The original text is kept below
because the reasoning is still the right shape, and because it is a fair
warning about deducing the contents of something one could simply measure.

Two things follow, and the second matters more than the first:

1. `voice::FieldKey` is supplied by the caller and no default is offered.
   Choosing the number is a protocol decision, not an implementation detail.
2. **FIELD_AUDIO is probably the wrong home for a clip regardless.** Its mode
   enumeration is LXMF's own, covering Codec2 and Opus; Pipit's codecs are not
   in it and will not be. A stock client that recognised the number would hand
   a Pipit clip to a decoder expecting a different codec and render noise.
   That is the same parasitism the
   [mesh household doc](2026-07-20_mesh_household_tulle_tucket_sennet.md)
   forbids for undecodable frames on a foreign mesh, and the argument against
   it does not depend on knowing the number.

So `voice::find_clip` locates a clip by its own self-describing header rather
than by field number, which means two Retinue peers exchange voice without
having agreed a number at all, and a stock client sees a field it does not
recognise and carries it opaquely, which is behaviour outrider's codec already
guarantees. Whether to additionally emit into the stock audio field is a
deliberate interop decision, and it needs the capture plus a mode value that
does not lie about the codec.

## Deployment shapes

- **Endpoint-coded, relay-carried (v1):** memos are recorded and decoded on
  hosts (turnstone, signalman) and phones-adjacent devices; field nodes only
  carry bytes. The codec never enters firmware, which keeps v1 small.
- **Field recorder (later):** a button-and-mic node encodes on-device. This
  is where no_std and the FPU budget actually bind, and it is the charming
  product shape. It waits for Rung 1 to exist.
- **Live or push-to-talk call (later):** endpoints stream Pipit frames through
  a bearer session. This needs a separate measured cadence, loss, recovery,
  and airtime proof; the codec bitrate alone does not prove that a given LoRa
  path can sustain a call.

## Naming

**Chosen: Pipit**, with package and repository name `pipit`. A pipit is a small
calling bird; the name puts the emphasis on a small voice carried through the
air, covering both recorded drops and live calls without suggesting that the
encoding warps the voice. Working descriptor:

> A small speech codec for voices that travel light.

**Quaver** was rejected because its second meaning foregrounds tremor and can
make the codec sound like an effect or damaged encoding.

**Purling**, **Mora**, and **Notelet** remain liked but unassigned names. They
may fit independently useful component crates later: Purling around continuous
frame flow, Mora around speech timing or frame primitives (a mora is the
linguistics unit of syllable timing, so the fit is exact), and Notelet around
the recorded-message envelope. These are a naming bench, not a reason to split
Pipit preemptively. Found a component only when it owns a durable API and has a
real second consumer. `waveshaper` remains reserved for a possible future
first-party device per the naming register.

Wider bench from the 2026-08-13 rounds, all verified free on crates.io that
day: **chirrup** (a small run of chirps; LoRa's PHY is chirp spread spectrum),
**cooee** (the Australian bush call; "within cooee" = within range),
**hollo** and **halloo** (the long-distance hail family), **silbo** (the
whistled ravine-speech of La Gomera; names a living practice, use with care),
**yodel** (rejected for this crate: implies deliberate warble, plus a UK
courier brand), **peewit** and **kittiwake** (birds named for their own
calls), **whoop**, and from the water round **syrinx** (Pan's reed pipes and
the avian voice organ), **plash**, **shallows**, **comber**, **ghyll**,
**haar**, **naiad**, **undine**. `pipit`, `mora`, and `notelet` themselves
were verified free 2026-08-13. Taken that day, for the record: holler, purl,
peep, chit, billet, missive, and essentially every plain water noun (rill,
beck, tarn, burble, bittern, swash, trickle, eddy, weir, shoal, sluice).
