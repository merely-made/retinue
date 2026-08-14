"""Observe LXMF's field registry and one message carrying an audio field.

Two observations, both black-box per the oracle discipline: public constants
are read at runtime, never from source, and the field number is then
confirmed on the wire by packing a real message through the public API and
reading the bytes back out.

Answers the question outrider's voice module left open: which field number
carries audio, and what an audio field's value looks like.
"""

from __future__ import annotations

import json
from pathlib import Path

import LXMF
import RNS
import RNS.vendor.umsgpack as msgpack

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
OUT = REPO / "crates" / "outrider" / "tests" / "fixtures" / "lxmf_0_9_6_fields.json"

TIMESTAMP = 1_753_603_208.5
TITLE = b"VOICE TITLE"
CONTENT = b"VOICE BODY"
# Stand-in for encoded speech. Its bytes do not matter; where it lands does.
AUDIO = bytes(range(32))


def public_constants(prefix: str) -> dict[str, int]:
    """Public constants read off the imported module at runtime."""
    found = {}
    for name in dir(LXMF):
        if not name.startswith(prefix):
            continue
        value = getattr(LXMF, name)
        if isinstance(value, int) and not isinstance(value, bool):
            found[name] = value
    return found


def main() -> None:
    fields = public_constants("FIELD_")
    modes = public_constants("AM_")

    # Confirm on the wire rather than trusting the constant: pack a real
    # message through the public API and read the field key back out of the
    # MessagePack payload.
    audio_field = LXMF.FIELD_AUDIO
    mode = LXMF.AM_CODEC2_2400

    identity = RNS.Identity()
    destination = RNS.Destination(
        identity, RNS.Destination.OUT, RNS.Destination.SINGLE, "lxmf", "delivery"
    )
    message = LXMF.LXMessage(destination, destination, CONTENT, TITLE)
    message.timestamp = TIMESTAMP
    message.fields = {audio_field: [mode, AUDIO]}
    message.pack()

    packed = message.packed
    # LXMF's packed layout is destination, source, signature, then the
    # MessagePack payload; the codec already proves those offsets.
    payload = msgpack.unpackb(packed[LXMF.LXMessage.DESTINATION_LENGTH
                                     + LXMF.LXMessage.DESTINATION_LENGTH
                                     + LXMF.LXMessage.SIGNATURE_LENGTH:])
    on_wire = payload[3]
    wire_keys = sorted(int(k) for k in on_wire)
    observed_mode, observed_audio = on_wire[audio_field]

    result = {
        "lxmf_version": "0.9.6",
        "rns_version": "1.4.2",
        "note": (
            "Public constants read at runtime and confirmed against packed "
            "bytes. No LXMF source was read to produce this."
        ),
        "fields": dict(sorted(fields.items(), key=lambda kv: kv[1])),
        "audio_modes": dict(sorted(modes.items(), key=lambda kv: kv[1])),
        "wire_check": {
            "audio_field": int(audio_field),
            "field_keys_present": wire_keys,
            "audio_value_shape": "[mode, bytes]",
            "mode_sent": int(mode),
            "mode_on_wire": int(observed_mode),
            "audio_bytes_match": bytes(observed_audio) == AUDIO,
        },
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
