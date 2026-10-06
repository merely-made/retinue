"""Compare a routing result against the previous pin's, cell by cell.

Usage: python compare_routes.py <old result.json> <new result.json> <output.json>

Cells are matched by their `cell.cell_id`. Only the decision fields the 1.5.4 and
1.5.6 receipts compared are compared here; timestamps and hashes differ per run.
"""

import hashlib
import json
from pathlib import Path
import sys

REPO = Path(__file__).resolve().parents[3]
FIELDS = [
    "blob_relation",
    "new_blob_admitted",
    "observable_admission",
    "same_blob_route_transition",
    "observed_hop_relation",
    "outcome",
    "valid_measurement",
]


def load(path):
    data = json.loads(path.read_text(encoding="utf-8"))
    return {cell["cell"]["cell_id"]: cell for cell in data["cells"]}


def main():
    old_path, new_path, out_path = (Path(arg).resolve() for arg in sys.argv[1:4])
    old, new = load(old_path), load(new_path)
    assert old.keys() == new.keys(), "cell sets differ"
    differing = [
        {"cell": name, "field": field, "old": old[name][field], "new": new[name][field]}
        for name in sorted(old)
        for field in FIELDS
        if old[name][field] != new[name][field]
    ]
    result = {
        "old_result": old_path.relative_to(REPO).as_posix(),
        "old_sha256": hashlib.sha256(old_path.read_bytes()).hexdigest(),
        "new_result": new_path.relative_to(REPO).as_posix(),
        "new_sha256": hashlib.sha256(new_path.read_bytes()).hexdigest(),
        "fields_compared": FIELDS,
        "cell_count": len(new),
        "all_equal": not differing,
        "differing_cells": differing,
    }
    Path(out_path).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"{len(new)} cells, all_equal={not differing}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
