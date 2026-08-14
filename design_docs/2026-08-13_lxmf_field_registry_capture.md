# LXMF field registry capture

**Date:** 2026-08-13
**Status:** Receipt. Closes the field-number question the
[lofi voice codec scoping note](2026-08-13_lofi_voice_codec_scoping.md) left
open, and **corrects a conclusion recorded there earlier the same day**.
Fixture: `crates/outrider/tests/fixtures/lxmf_0_9_6_fields.json`. Script:
`crates/outrider/oracle/capture_fields.py`.

## Why a capture was needed

Outrider's v1 scope admits field numbers "from captures and public prose"
only. LXMF's README states that full protocol documentation is still planned
and does not carry the registry, and the audio mode list is published only as
source, which this workspace does not read. So the number could not be taken
from prose, and the voice module shipped with a caller-supplied field key and
no default.

## What was run

`capture_fields.py` against pinned LXMF 0.9.6 and RNS 1.4.2, entirely on the
host: no radio, no COM port. Two observations, both inside the oracle
discipline, which permits "running it, calling its documented API, inspecting
its public constants at runtime, and reading its output":

1. Public `FIELD_*` and `AM_*` constants read off the imported module at
   runtime.
2. The number confirmed on the wire, by packing a real message through the
   public API and reading the field key back out of the MessagePack payload.

The second step is what makes this a capture rather than a claim. Both agree.

## What it found

Fields 1 through 15 are assigned, with a custom range at the top:

| Field | Number |
| --- | --- |
| EMBEDDED_LXMS | 1 |
| TELEMETRY | 2 |
| TELEMETRY_STREAM | 3 |
| ICON_APPEARANCE | 4 |
| FILE_ATTACHMENTS | 5 |
| IMAGE | 6 |
| **AUDIO** | **7** |
| THREAD | 8 |
| COMMANDS | 9 |
| RESULTS | 10 |
| GROUP | 11 |
| TICKET | 12 |
| EVENT | 13 |
| RNR_REFS | 14 |
| RENDERER | 15 |
| CUSTOM_TYPE | 251 |
| CUSTOM_DATA | 252 |
| CUSTOM_META | 253 |
| NON_SPECIFIC | 254 |
| DEBUG | 255 |

An audio field's value is a two-element list, `[mode, bytes]`. Modes are
Codec2 at 1 through 9, Opus at 16 through 25, and **`AM_CUSTOM` at 255**.

## The correction

The scoping note concluded, before this capture, that the stock audio field
was "probably the wrong home for a clip regardless", on the reasoning that
its mode enumeration is Codec2 and Opus, so a client recognising the number
would hand a Pipit clip to the wrong decoder and render noise. That reasoning
was sound on what was known and **the conclusion is wrong**, because the
enumeration is not closed: `AM_CUSTOM` means precisely "audio in a codec
outside this list", which is exactly what a Pipit clip is.

So clips now ride field 7 with mode `AM_CUSTOM`. The message reads as voice
to any client, no client is invited to misdecode it, and the clip's own
header says which codec it actually is. That is strictly better than the
custom-data route, which would have carried the bytes while losing the fact
that they are speech.

Worth keeping in view: the pre-capture reasoning was careful and still landed
on the wrong answer, because it was reasoning about the shape of a registry
nobody had looked at. The capture cost one script and no hardware.

## What did not change

`voice::find_clip` still locates a clip by Pipit's own self-describing header
rather than by field number, so two Retinue peers interoperate without having
agreed a field at all. The field number is now a sensible default rather than
the only route in.

`voice::audio_at` reads any audio field and returns its declared mode, so a
stock Codec2 message can be reported as voice-we-cannot-decode rather than
silently ignored. Decoding Codec2 itself remains out of the question here for
the reasons in the
[Rung 2 decision](2026-08-13_rung2_codec2_class_decision.md).

## Reproducing

```sh
crates/retinue/oracle/.venv/Scripts/python.exe crates/outrider/oracle/capture_fields.py
```

The venv wants `lxmf==0.9.6` beside the pinned `rns==1.4.2`.
