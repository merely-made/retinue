# pipit

A small speech codec for voices that travel light.

Pipit is maintained in
[Retinue](https://github.com/merely-made/retinue/tree/main/crates/pipit) as
`crates/pipit`; its own repository was retired on 2026-11-25 and its history
came along.

## Status (2026-11-26)

Published as `pipit` 0.2.1, which changes only where the crate lives; the
codec is the one 0.2.0 shipped. All three rungs of the plan are implemented
and tested.

Three codecs, one for each kind of link:

- **IMA ADPCM**, 4 bits per sample, about 32 kbps. A cheaper description of
  the waveform, so it still sounds like the recording. Measured 34.8 dB SNR
  in steady state, converging within 10 ms from a cold start. Sized for
  WiFi and TCP bearers.
- **LPC-10**, a 10th-order vocoder at 2,489 bps, thirteen times smaller.
  Transmits no waveform at all: ten reflection coefficients, a pitch, a
  gain, and a voicing bit per 22.5 ms, from which speech is rebuilt. Sized
  for LoRa. Measured 1.97 dB mean log spectral distortion, pitch recovered
  within one quantizer step across 80 to 400 Hz, and voiced, unvoiced, and
  silent frames classified correctly.
- **LPC-10 half rate**, 1,600 bps, a third less airtime again. The vocal
  tract is sent once per two frames and interpolated between, because the
  tract moves slowly while pitch and energy move fast. Measured cost is
  0.03 dB of spectral distortion on steady speech and 0.02 dB on a fast
  moving tract, for 36% fewer bits. Pitch and voicing are untouched, being
  still sent every frame.

Shared properties:

- **Frames describe themselves,** so a frame lost mid-call costs its own few
  milliseconds and nothing after it. An isolated ADPCM frame measures within
  0.5 dB of the same frame in a continuous stream; a vocoder frame carries a
  complete parametric description of its own audio.
- **Clips** are the stored form: an 18-byte header naming codec, mode,
  sample rate, and frame geometry, then frames back to back. Ten seconds of
  8 kHz speech is 40 KB as ADPCM, 3.1 KB through the vocoder, and 2.0 KB at
  half rate.
- `#![no_std]`, `#![forbid(unsafe_code)]`, zero dependencies, including the
  float math the vocoder needs. Verified building for
  `thumbv7em-none-eabihf` (nRF52840) and `riscv32imac-unknown-none-elf`
  (ESP32-C-class) as well as the host.
- **Decoding is treated as a hostile-input surface.** Malformed headers,
  truncated clips, and invalid coder state return errors rather than
  panicking, and no declared length sizes a buffer on its own. Every 7-byte
  pattern is a valid vocoder frame that yields a stable filter, and the
  decoder recovers to correct output after arbitrary garbage.

Codec2 bitstream compatibility was investigated and ruled out rather than
deferred: Codec2's low modes quantize against trained codebooks published
only under LGPL, and a decoder cannot read the bitstream without the exact
codebook the encoder searched, so an implementation written from the
literature cannot reach compatibility at all. FFmpeg, which prefers native
decoders, still requires libcodec2 for the same reason. Interop, if it is
ever wanted, belongs in an application that links the real library under a
compatible licence, not in this crate. The reasoning is recorded in Retinue's
`design_docs/2026-08-13_rung2_codec2_class_decision.md`, alongside the rung
plan in `design_docs/2026-08-13_lofi_voice_codec_scoping.md`.

### On FIPS-137

The vocoder is built to the *structure* of LPC-10e, the 2.4 kbps US federal
standard: same frame length, model order, parameter set, and bit allocation.
The quantizer tables are this crate's own, because the standard's tables
were not available to implement from. **A Pipit vocoder frame is therefore
not interchangeable with a FIPS-137 frame**, and no such interoperability is
claimed. A conformant variant, if ever wanted, takes a new codec identifier
rather than changing this one.

## Use

First consumers are voice messages carried store-and-forward over a mesh,
and later live or push-to-talk calls over the same paths. Usable from any
Rust project that needs speech in few bytes.

```sh
cargo add pipit                            # host: clips and frames
cargo add pipit --no-default-features      # firmware: frames only, no alloc
```

Whole clips, which is what a recorded message is. Pick the codec by the
link you are sending over:

```rust
let clip = pipit::encode_clip(&pcm, pipit::ClipParams::lpc10_half())?;  // tight LoRa
let clip = pipit::encode_clip(&pcm, pipit::ClipParams::lpc10())?;       // LoRa
let clip = pipit::encode_clip(&pcm, pipit::ClipParams::adpcm())?;       // WiFi
let (header, pcm) = pipit::decode_clip(&clip)?;                         // any of them
```

Frame at a time, which is what a call drives:

```rust
let mut encoder = pipit::lpc10::Encoder::new();
let mut decoder = pipit::lpc10::Decoder::new();
encoder.encode_frame(&pcm_frame, &mut buffer)?;
decoder.decode_frame(&buffer, &mut out_frame)?;
```

A clip is only the payload: no sender, recipient, timestamp, or signature.
Those belong to whatever envelope carries it, which is what keeps a clip
decodable by a host with no radio stack.

A vocoder cannot be judged by comparing waveforms: it discards the waveform
and builds a new one carrying the same message, so sample-by-sample error is
large by design and means nothing. The tests measure what it does preserve,
which is the spectral envelope, pitch, voicing, and energy.

## Relationship to IMA ADPCM

The nibble arithmetic is the standard IMA/DVI algorithm, implemented from
its public description. The framing is this crate's own and is deliberately
not IMA WAV's block layout, so nothing here reads or writes IMA WAV files.

## License

MIT OR Apache-2.0.

---

*This README was generated by AI and will be edited by the author upon
release.*

## Local checks

Enable the commit checks with `prek install --allow-missing-config`; run
`prek run --all-files` alongside the existing local gates. Check definitions
live in `.pre-commit-config.yaml`, with source-size exclusions and exceptions
in `line-ceiling.toml`. Shared house style and rollout: `mere/design_docs/mere_docs/implementation_strategy/2026-10-30_house_style_plan.md`.
