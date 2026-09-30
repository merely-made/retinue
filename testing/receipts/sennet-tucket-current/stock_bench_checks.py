"""Check RF receipt admission against captured bytes without opening ports."""

import importlib.util
import json
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "testing"))
spec = importlib.util.spec_from_file_location("stock_bench", ROOT / "testing/sennet_stock_bench.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def main():
    receipt = json.loads((Path(__file__).parent / "physical/sennet-stock-duplicate-settled.json").read_text())
    frame = bytes.fromhex(receipt["matching_client_frames"][0])
    source, packet_id = int(receipt["source"], 16), int(receipt["packet_id"], 16)
    text = receipt["text"]
    assert bench.matching(frame, source, text, packet_id)
    assert not bench.matching(frame, source ^ 1, text, packet_id)
    assert not bench.matching(frame, source, text + "!", packet_id)
    assert not bench.matching(frame, source, text, packet_id + 1)
    assert not bench.matching(frame[:10], source, text, packet_id)
    framed = b"\x94\xc3" + len(frame).to_bytes(2, "big") + frame
    assert bench.deframe(b"noise" + framed + b"\x94\xc3\x01") == [frame]
    # A serial interruption must retain the child's completed transmit output,
    # not silently discard the evidence while terminating an unfinished child.
    class Radio:
        def __enter__(self): return self
        def __exit__(self, *args): pass
        def open(self): pass
        def reset_input_buffer(self): pass
        def write(self, data): return len(data)

    class Child:
        returncode = None
        killed = False
        def poll(self): return self.returncode
        def kill(self):
            self.killed = True
            self.returncode = 1
        def communicate(self): return "transmitted packet before USB interruption", None

    child, written = Child(), []
    arguments = ["bench", "--phy", "SIMULATED", "--stock", "SIMULATED",
                 "--examples", "unused", "--state", "unused",
                 "--producer-evidence", "unused", "--producer-version", "simulated",
                 "--text", "simulated", "--output", "unused"]
    with patch.object(sys, "argv", arguments), \
         patch.object(Path, "exists", return_value=False), \
         patch.object(Path, "read_bytes", return_value=b"simulated"), \
         patch.object(Path, "write_text", side_effect=lambda value, **kwargs: written.append(json.loads(value))), \
         patch.object(bench.serial, "Serial", return_value=Radio()), \
         patch.object(bench.subprocess, "Popen", return_value=child), \
         patch.object(bench, "collect", side_effect=[[], OSError("simulated USB interruption")]), \
         patch.object(bench.time, "sleep"):
        assert bench.main() == 1
    assert child.killed and not written[0]["passed"]
    assert written[0]["rust_exit"] == 1 and "transmitted" in written[0]["rust_output"]
    assert "simulated USB interruption" in written[0]["error"]
    print("6 captured-byte admission cases and 1 serial-interruption case passed; no serial ports opened")


if __name__ == "__main__":
    main()
