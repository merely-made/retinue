"""Check RF receipt admission against captured bytes without opening ports."""

import importlib.util
import json
from pathlib import Path
import sys

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
    print("6 captured-byte admission cases passed; no serial ports opened")


if __name__ == "__main__":
    main()
