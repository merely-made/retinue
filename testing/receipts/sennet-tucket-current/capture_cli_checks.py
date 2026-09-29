"""Exercise capture CLI evidence/failure behavior using a simulated serial endpoint."""

import hashlib
import json
from pathlib import Path
import runpy
import sys
import types
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "crates/sennet/capture/capture_config.py"


class Radio:
    def __init__(self, continuous=False, empty=False):
        self.continuous = continuous
        self.payload = b"" if empty else b"\x94\xc3\x00\x02\x08\x01"

    def read(self, count):
        if self.continuous:
            return (self.payload * ((count // len(self.payload)) + 1))[:count]
        payload, self.payload = self.payload, b""
        return payload

    def reset_input_buffer(self): pass
    def write(self, data): return len(data)
    def flush(self): pass
    def close(self): pass


def capture(arguments, radio):
    output = []
    ticks = iter(index / 2 for index in range(100))
    endpoint = types.SimpleNamespace(Serial=lambda *args, **kwargs: radio)
    with patch.dict(sys.modules, {"serial": endpoint}), \
         patch.object(sys, "argv", [str(SCRIPT), "SIMULATED", "unused.json", "simulation", *arguments]), \
         patch.object(Path, "exists", return_value=False), \
         patch.object(Path, "read_bytes", return_value=b"simulated device identification"), \
         patch.object(Path, "write_text", side_effect=lambda self_text, **kwargs: output.append(json.loads(self_text))), \
         patch("time.sleep"), patch("time.time", side_effect=lambda: next(ticks)), \
         patch("time.monotonic", return_value=0):
        try:
            runpy.run_path(str(SCRIPT), run_name="__main__")
            status = 0
        except SystemExit as error:
            status = error.code
    return status, output


def main():
    status, output = capture([], Radio())
    assert status == 0 and output[0]["producer"]["version"] is None
    status, output = capture(["--producer-version", "2.7.26.54e0d8d", "--producer-evidence", "identification.txt"], Radio())
    assert status == 0 and output[0]["producer"]["version"] == "2.7.26.54e0d8d"
    assert output[0]["producer"]["evidence_sha256"] == hashlib.sha256(b"simulated device identification").hexdigest()
    status, output = capture([], Radio(empty=True))
    assert status != 0 and output[0]["capture_status"] == "no_frames"
    status, output = capture([], Radio(continuous=True))
    assert status != 0 and output[0]["stop_reason"] == "byte_limit"
    status, output = capture(["--producer-version", "2.7.26.54e0d8d"], Radio())
    assert status == 2 and output == []
    print("5 simulated CLI cases passed; no serial port opened or capture file written")


if __name__ == "__main__":
    main()
