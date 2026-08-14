# Rung 2 decision: what a Codec2-class coder can and cannot be here

**Date:** 2026-08-13
**Status:** Decision doc, called for by the
[lofi voice codec scoping note](2026-08-13_lofi_voice_codec_scoping.md),
which gated Rung 2 behind exactly this document. It closes the rung as
originally framed and records what replaced it.

**Option A shipped the same day** as `pipit::lpc10::half`, codec id 3:
1,600 bps against the full rate's 2,489, measured at 0.03 dB of extra
spectral distortion on steady speech and 0.02 dB on a fast moving tract. See
the measured results at the end.

## What Rung 2 was supposed to be

The scoping note put it this way: better quality per bit, and "bitstream
compatibility with Codec2 would buy interop with the existing amateur
ecosystem (FreeDV, M17 uses Codec2 modes)". It named the obstacle as effort:
"an independent implementation from Rowe's publications verified black-box
against the reference, which is a genuinely large DSP project."

That framing was too optimistic. The obstacle is not effort.

## Codec2 bitstream compatibility inside Pipit is closed

Three findings, in increasing order of how final they are.

**The licence bars the code.** Codec2 is LGPL-2.1, and retinue's `deny.toml`
red-lines LGPL for the dependency tree, because it would relicense any
application linking the library crates. That was already known. It rules out
the Rust `codec2` crate and a translation of the C reference alike, and it is
the *weakest* of the three findings because it constrains only where the code
may live.

**The tables are the format, and the tables are the source.** Codec2's lower
modes quantize with multi-stage vector quantization against trained
codebooks, shipped as arrays of floats in the LGPL tree. A decoder cannot
interpret a Codec2 bitstream without the exact codebook the encoder searched:
the bits are indices into it. Trained codebooks are empirical artefacts of a
training corpus and a training run. They cannot be re-derived from a
description of the method, which is what "implement from the publications"
would give us. So for those modes, an independent implementation does not
merely cost more; it cannot reach compatibility at all without copying the
data it is not allowed to copy.

**Nobody has done it.** FFmpeg, which prefers native decoders as a matter of
policy and has the deepest bench in the field for exactly this kind of work,
states plainly: "There is currently no native decoder, so libcodec2 must be
used for decoding." If a native Codec2 decoder were a tractable prize, it
would exist there already. Our declining it is not timidity.

The conclusion is not "this is hard." It is that Codec2 compatibility is not
something Pipit can hold, on any schedule, without becoming a derivative of a
licence the workspace has ruled out. **Rung 2 as originally framed is closed.**

## If M17 or FreeDV interop is genuinely wanted, it lives outside

The interop goal is real even though this route to it is not. The correct
shape was already named in the scoping note as a "forever-external
component", and it is worth stating precisely, because one detail makes it
much easier than it sounds:

**Retinue's firmware images are GPLv3, and LGPL is GPL-compatible.** So
libcodec2 can live inside a firmware image, or inside a GPL-licensed host
application, without touching the MPL library tree or Pipit at all. The
boundary that must hold is the one `deny.toml` guards: the crates that
mere and turnstone consume. An application at the edge of the stack is not
that.

So M17 interop, if pursued, is an application-tier project that links the
real thing, not a codec project inside Pipit. It would also want M17's
framing and modulation, which is a far larger surface than the codec and has
nothing to do with this rung. That is its own plan if the day comes.

## What should replace Rung 2

Pipit's own network does not need Codec2. It needs the two things the rung
was really about, which turn out to be separable, and only one of them is
cheap:

**Option A, less airtime: a half-rate superframe mode.** The vocoder's 53
bits split as 41 for the spectrum and 12 for pitch, gain and voicing. The
spectrum is what dominates, and it changes slowly. A superframe covering two
22.5 ms frames would carry the spectrum once and per-frame pitch, gain and
voicing twice: 41 + 24 = 65 bits, or 9 bytes per 45 ms with byte alignment,
which is 1,600 bps against the present 2,489. Ten seconds falls from 3.1 KB
to 2.0 KB, a 36% airtime cut on a duty-cycle-limited link. It needs no
trained data and no new analysis; it is a repacking plus interpolation, and
it lands as codec id 3 behind the clip header the container already carries.

Its ceiling is honest: the spectrum still costs 41 scalar-quantized bits.
Getting materially below 1,200 bps means vector-quantizing the spectrum,
which means training codebooks on a speech corpus. That is a real project
with a data-collection problem attached, and it should not be started
casually. It is also the one place where our own trained codebooks would be
our own, with no licence question at all.

**Option B, less buzz: better excitation, decoder-side only.** The synthetic
timbre comes from driving the filter with a bare impulse train. Pulse
dispersion, small pitch jitter, and an adaptive postfilter are all pre-1980
techniques that reduce it, and all three are decoder-side: they change how
existing parameters are rendered, not what is transmitted. That means every
clip already encoded improves, with no new codec id and no format break.

Its ceiling is also honest: without per-band voicing strengths in the
bitstream there is a limit to how natural a two-state excitation can sound,
and adding those strengths would be a format change.

**Recommendation: A, and treat B as optional polish.** The scoping note
already ruled that "the robotic timbre is on-brand for a radio product; lofi
is the point", so naturalness is not the product's constraint. Airtime is.
A duty-cycle-limited LoRa link is the thing that actually bounds how much
voice the network can carry, and Option A cuts it by a third for a
contained, well-understood piece of work.

## What Option A actually measured

Landed as `pipit::lpc10::half`, 9 bytes per 45 ms superframe, codec id 3 in
the clip header. Ten seconds of speech falls from 3.1 KB to 2.0 KB.

The interesting result is how little it costs. Full rate against half rate,
same signal, same instrument:

| Material | Full rate | Half rate |
| --- | --- | --- |
| Steady vowel | 1.99 dB | 2.02 dB |
| Fast moving tract | 3.76 dB | 3.78 dB |

Two hundredths of a decibel for a third of the airtime, even when the tract
is moving. The reason is that the synthesiser already interpolates
parameters across four subframes in *both* modes, so full rate was smoothing
the spectral trajectory anyway; half rate only lengthens the span it
interpolates over.

Three caveats belong with those numbers rather than in a footnote:

- The signals are synthetic. Real speech with plosives and stop consonants
  moves faster than a resonator sweep, and nothing here has been measured
  against a human talker.
- Half rate samples the spectrum on even frames only, so articulation that
  moves in step with the superframe can be systematically flattered or
  missed. An early version of the test alternated vowels every frame and had
  half rate scoring *better* than full rate purely from that lock. The test
  now sweeps on an odd five-frame cycle, and the direction of the comparison
  is deliberately not asserted.
- Comparing distortion across two different signals says nothing, because it
  conflates how hard a spectrum is to quantize with what interpolation
  costs. Only full-against-half on one signal is meaningful, which is what
  the table reports.

Given how cheap the trade turned out, half rate is a reasonable default for
LoRa bearers rather than an exceptional setting. Full rate remains right
where a link has room, and the clip header names which was used, so a
receiver never needs telling.

## Stop rules

- Do not translate, vendor, or depend on Codec2 or its codebooks inside any
  crate the library tree can reach, including Pipit.
- Do not describe a future Pipit mode as Codec2-compatible. It will not be.
- Do not start vector quantization of the spectrum without first deciding
  where the training speech comes from and under what terms.
- If M17 comes up, it is an application-tier project linking libcodec2 under
  GPL, not a Pipit mode.
