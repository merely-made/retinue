"""Stock RNS 1.6 versus Retinue with network stamps enabled and enforced.

Each case uses an isolated loopback-only stock profile and a prebuilt Rust peer.
Cases cover both emission directions, authenticated app data/ratchets, stamped
path discovery, links in both directions, and legacy/insufficient announce refusal.
The 1.6.1 cases run Retinue on its default policy against an enforcing stock peer,
across a restart, and with a destination stamp mined by stock RNS.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time


CASES = ("enforced", "legacy", "stock-refuses-legacy", "retinue-refuses-legacy",
         "stock-refuses-low", "retinue-refuses-low", "retinue-refuses-legacy-path",
         "retinue-refuses-low-path", "stock-refuses-low-path", "retinue-refuses-low-link",
         "defaults-enforced", "defaults-restart", "stock-mined-stamp")
SLOW_CASES = {"defaults-enforced": 300, "defaults-restart": 300, "stock-mined-stamp": 120}
STOCK_VERSIONS = ("1.6.0", "1.6.1")
RUST_SECRET = bytes([0x47]) * 64
STOCK_APP = b"stock\xc1authenticated"
RUST_APP = b"rust\xc1authenticated"


def wait(predicate, seconds=10):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if predicate():
            return True
        time.sleep(0.05)
    return bool(predicate())


class Peer:
    def __init__(self, binary: Path, emit, enforce: bool, cost: float,
                 path_cost: float = 2.5, storage: Path | None = None):
        self.lines = []
        self.started = time.monotonic()
        mode = emit if isinstance(emit, str) else "emit" if emit else "plain"
        self.process = subprocess.Popen(
            [str(binary), "0", mode,
             "enforce" if enforce else "permissive", str(round(cost * 256)),
             str(round(path_cost * 256)), "896", *([str(storage)] if storage else [])],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, bufsize=1)
        threading.Thread(target=self.pump, daemon=True).start()

    def pump(self):
        for line in self.process.stdout:
            self.lines.append(line.rstrip())
            print("[retinue]", line.rstrip(), flush=True)

    def command(self, text):
        self.process.stdin.write(text + "\n")
        self.process.stdin.flush()

    def matching(self, pattern, seconds=10):
        found = []
        def check():
            found[:] = [m for line in self.lines if (m := re.fullmatch(pattern, line))]
            return bool(found)
        assert wait(check, seconds), (pattern, self.lines)
        return found[-1]

    def close(self):
        if self.process.poll() is None:
            self.command("QUIT")
            try:
                self.process.wait(5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(5)


def stock_config(root: Path, port: int, emit: bool, enforce: bool, required: float,
                 value: float, path_cost: float):
    flag = lambda value: "Yes" if value else "No"
    (root / "config").write_text(
        "[reticulum]\nshare_instance=No\nenable_transport=Yes\n"
        "panic_on_interface_error=No\n[logging]\nloglevel=2\n[stamps]\n"
        f"stamp_destinations={flag(emit)}\nstamp_prs={flag(emit)}\n"
        f"enforce_stamps={flag(enforce)}\ndestination_cost={required}\n"
        f"destination_value={value}\npr_cost={path_cost}\npr_value=2.5\nlr_cost=3.5\n"
        "lr_stamp_timeout=5\npr_stamp_timeout=5\n[interfaces]\n[[peer]]\n"
        "type=TCPClientInterface\nenabled=Yes\ntarget_host=127.0.0.1\n"
        f"target_port={port}\n", encoding="ascii")


def start_stock(root: Path, port: int, emit: bool, enforce: bool, required: float,
                value: float, path_cost: float):
    import RNS
    stock_config(root, port, emit, enforce, required, value, path_cost)
    RNS.Reticulum(configdir=str(root))
    # 1.6.1 stamps by default and its config can only switch stamping on.
    RNS.Transport.STAMP_DESTINATIONS = emit
    RNS.Transport.STAMP_PRS = emit
    assert wait(lambda: any(i.online for i in RNS.Transport.interfaces))


def stamp_file(storage: Path, destination: bytes) -> Path:
    return storage / "stamps" / f"{destination.hex()}.dstamp"


def mine(root: Path) -> None:
    """Mine the Rust peer's destination stamp with stock RNS, offline, at value 8."""
    import RNS
    (root / "config").write_text("[reticulum]\nshare_instance=No\n[logging]\nloglevel=2\n"
                                 "[interfaces]\n", encoding="ascii")
    RNS.Reticulum(configdir=str(root))
    RNS.Transport.STAMP_DESTINATIONS = True
    # Destination.set_value can only raise the target above the global value.
    RNS.Transport.DESTINATION_COST = RNS.Transport.DESTINATION_VALUE = 8.0
    destination = RNS.Destination(RNS.Identity.from_bytes(RUST_SECRET), RNS.Destination.IN,
                                  RNS.Destination.SINGLE, "retinue", "network-stamp")
    destination.set_value(8.0)
    assert destination.announce(send=False) is not None
    assert stamp_file(Path(RNS.Reticulum.storagepath), destination.hash).is_file()
    RNS.exit()


def defaults(binary: Path, name: str, root: Path) -> dict:
    """Retinue's default policy against stock 1.6.1 defaults with enforcement on."""
    import RNS
    results = {}
    storage = root / "rust-storage"
    if name == "defaults-restart":
        cold = Peer(binary, "default", False, 0.0, storage=storage)
        try:
            destination = bytes.fromhex(cold.matching(r"DEST ([0-9a-f]{32})")[1])
            assert wait(lambda: stamp_file(storage, destination).is_file(), 240)
            results["cold mining seconds"] = round(time.monotonic() - cold.started, 2)
        finally:
            cold.close()
        stored = stamp_file(storage, destination).read_bytes()
    elif name == "stock-mined-stamp":
        miner = root / "miner"
        miner.mkdir()
        subprocess.run([sys.executable, "-u", __file__, "--mine", str(miner)], check=True,
                       timeout=90)
        storage = miner / "storage"
    if name == "stock-mined-stamp":
        peer = Peer(binary, True, False, 8.0, storage=storage)
    else:
        peer = Peer(binary, "default", False, 0.0, storage=storage)
    try:
        port = int(peer.matching(r"LISTENING (\d+)")[1])
        rust_destination = bytes.fromhex(peer.matching(r"DEST ([0-9a-f]{32})")[1])
        if name == "stock-mined-stamp":
            stored = stamp_file(storage, rust_destination).read_bytes()
            start_stock(root, port, True, True, 8.0, 8.0, 2.5)
        else:
            # Stock defaults: destination 23.5, path request 15.8 required.
            start_stock(root, port, True, True, 23.5, 25.0, 15.8)
        assert RNS.Transport.STAMPS_ENFORCED and RNS.Transport.LR_STAMPS_ENFORCED
        stock = RNS.Destination(RNS.Identity.from_bytes(bytes([0x38]) * 64),
                                RNS.Destination.IN, RNS.Destination.SINGLE,
                                "retinue", "network-stamp")
        # Retinue keeps a link cost only from an announce that meets its floor, as stock
        # does; on defaults that is stock's own value of 25.
        if name == "stock-mined-stamp":
            stock.set_value(8.0)
        stock.set_link_cost(3.5)
        stock.set_default_app_data(STOCK_APP)
        incoming = []
        stock.set_link_established_callback(incoming.append)
        # A fresh reader: stock has never heard the Rust destination.
        RNS.Transport.request_path(rust_destination)
        results["enforcing stock discovered Retinue"] = wait(
            lambda: RNS.Transport.has_path(rust_destination), 240)
        results["seconds to first accepted announce"] = round(time.monotonic() - peer.started, 2)
        out = RNS.Destination(RNS.Identity.recall(rust_destination), RNS.Destination.OUT,
                              RNS.Destination.SINGLE, "retinue", "network-stamp")
        link = RNS.Link(out)
        results["stock link to Retinue reached ACTIVE"] = wait(
            lambda: link.status == RNS.Link.ACTIVE, 30)
        peer.matching("INBOUND")
        link.teardown()
        peer.command(f"PATH {stock.hash.hex()}")
        peer.matching(rf"APP {stock.hash.hex()} {STOCK_APP.hex()} \S+ 896", 180)
        peer.command(f"LINK {stock.hash.hex()}")
        peer.matching(rf"ACTIVE {stock.hash.hex()}", 30)
        results["Retinue stamped link to enforcing stock"] = wait(lambda: bool(incoming))
        if name == "defaults-enforced":
            results["destination stamp persisted"] = stamp_file(storage, rust_destination).is_file()
        else:
            results["stored stamp reused unchanged"] = (
                stamp_file(storage, rust_destination).read_bytes() == stored)
            results["restart announced within 5 s"] = results["seconds to first accepted announce"] < 5
        print(json.dumps(results), flush=True)
        assert all(v for v in results.values() if isinstance(v, bool)), results
        return results
    finally:
        peer.close()


def case(binary: Path, name: str, root: Path) -> dict:
    import RNS
    assert RNS.__version__ in STOCK_VERSIONS, RNS.__version__
    if name in SLOW_CASES:
        assert RNS.__version__ == "1.6.1", "default-policy cases need stock 1.6.1"
        return defaults(binary, name, root)
    rust_emit = name not in ("legacy", "stock-refuses-legacy")
    rust_enforce = name == "enforced" or name.startswith("retinue-refuses")
    stock_emit = name not in ("legacy", "retinue-refuses-legacy",
                             "retinue-refuses-legacy-path", "retinue-refuses-low-path")
    stock_enforce = name == "enforced" or name.startswith("stock-refuses")
    rust_cost = 100.0 if name == "retinue-refuses-low" else 0.0 if name == "stock-refuses-low" else 4.5
    stock_required = 100.0 if name == "stock-refuses-low" else 4.5
    stock_value = 0.0 if name in ("stock-refuses-low", "retinue-refuses-low") else 4.5
    peer = Peer(binary, rust_emit, rust_enforce, rust_cost,
                path_cost=0.0 if name == "stock-refuses-low-path" else 2.5)
    try:
        port = int(peer.matching(r"LISTENING (\d+)")[1])
        rust_destination = bytes.fromhex(peer.matching(r"DEST ([0-9a-f]{32})")[1])
        start_stock(root, port, stock_emit, stock_enforce, stock_required, stock_value,
                    path_cost=100.0 if name == "stock-refuses-low-path" else 2.5)
        destination = RNS.Destination(RNS.Identity.from_bytes(bytes([0x38]) * 64),
                                      RNS.Destination.IN, RNS.Destination.SINGLE,
                                      "retinue", "network-stamp")
        destination.set_value(stock_value)
        destination.set_link_cost(3.5)
        destination.set_default_app_data(STOCK_APP)
        destination.enable_ratchets(str(root / "stock.ratchets"))
        incoming = []
        destination.set_link_established_callback(incoming.append)
        results = {}
        if name.startswith("retinue-refuses") and name.endswith("-path"):
            tag = bytes([0x26]) * 16
            interface = next(i for i in RNS.Transport.interfaces if i.online)
            extension = None
            if name == "retinue-refuses-low-path":
                from RNS import Stamps
                stamp = next(n.to_bytes(14, "big") for n in range(1000)
                             if not Stamps.stamp_valid(n.to_bytes(14, "big"), 2.5,
                                                       rust_destination + tag))
                assert not Stamps.stamp_valid(stamp, 2.5, rust_destination + tag)
                extension = b"\xc1\x00" + stamp
            RNS.Transport.request_path(rust_destination, on_interface=interface,
                                       tag=tag, ext_payload=extension)
            time.sleep(0.7)
            results["invalid path request received no announce"] = not RNS.Transport.has_path(rust_destination)
            _, extension = RNS.Transport.stamp_path_request(rust_destination, tag=tag,
                                                           return_payload=True)
            RNS.Transport.request_path(rust_destination, on_interface=interface,
                                       tag=tag, ext_payload=extension)
            results["valid same-tag request received announce"] = wait(
                lambda: RNS.Transport.has_path(rust_destination))
            print(json.dumps(results), flush=True)
            assert all(results.values()), results
            return results
        if name == "stock-refuses-low-path":
            requests = []
            inbound = RNS.Transport.inbound
            def observe_request(raw, *args, **kwargs):
                packet = RNS.Packet(None, raw)
                if packet.unpack() and packet.destination_hash == RNS.Transport.path_request_destination.hash:
                    requests.append(packet)
                return inbound(raw, *args, **kwargs)
            RNS.Transport.inbound = observe_request
            peer.command(f"PATH {destination.hash.hex()}")
            assert wait(lambda: bool(requests))
            time.sleep(0.7)
            results["stock saw Retinue path request"] = bool(requests)
            results["Retinue received no announce for insufficient work"] = not any(
                line.startswith(f"APP {destination.hash.hex()} ") for line in peer.lines)
            print(json.dumps(results), flush=True)
            assert all(results.values()), results
            return results
        if name.startswith("stock-refuses"):
            packets = []
            inbound = RNS.Transport.inbound
            def observe(raw, *args, **kwargs):
                packet = RNS.Packet(None, raw)
                if packet.unpack() and packet.destination_hash == rust_destination:
                    packets.append(packet)
                return inbound(raw, *args, **kwargs)
            RNS.Transport.inbound = observe
            peer.command(f"ANNOUNCE {RUST_APP.hex()}")
            assert wait(lambda: any(p.packet_type == RNS.Packet.ANNOUNCE for p in packets))
            time.sleep(0.5)
            results["stock rejects announce"] = not RNS.Transport.has_path(rust_destination)
            print(json.dumps(results), flush=True)
            assert all(results.values())
            return results
        if name.startswith("retinue-refuses") and not name.endswith("-link"):
            packet = destination.announce(app_data=STOCK_APP, send=False)
            packet.send()
            time.sleep(1)
            results["retinue rejects announce"] = not any(
                line.startswith(f"APP {destination.hash.hex()} ") for line in peer.lines)
            print(json.dumps(results), flush=True)
            assert all(results.values())
            return results

        # Discover a stock destination which has never emitted an announce.
        peer.command(f"PATH {destination.hash.hex()}")
        app = peer.matching(rf"APP {destination.hash.hex()} {STOCK_APP.hex()} ([0-9a-f]+) (\d+|none)")
        results["path request reached stock and returned an announce"] = bool(app)
        results["retinue authenticated app data and retained ratchet"] = bool(app[1])
        results["retinue learned fractional link cost"] = app[2] == ("896" if stock_emit else "none")
        peer.command(f"ANNOUNCE {RUST_APP.hex()}")
        assert wait(lambda: RNS.Transport.has_path(rust_destination))
        results["stock received authenticated app data"] = RNS.Identity.recall_app_data(rust_destination) == RUST_APP
        results["stock retained ratchet"] = RNS.Identity.get_ratchet(rust_destination) is not None
        results["stock learned fractional link cost"] = RNS.Identity.recall_link_cost(rust_destination) == (3.5 if rust_emit else None)
        if name == "retinue-refuses-low-link":
            from RNS import Stamps
            out = RNS.Destination(RNS.Identity.recall(rust_destination), RNS.Destination.OUT,
                                  RNS.Destination.SINGLE, "retinue", "network-stamp")
            sig_public = destination.identity.get_public_key()[32:]
            from RNS.Cryptography import X25519PrivateKey
            candidate = next(
                public for n in range(1000)
                if not Stamps.stamp_valid(
                    public := X25519PrivateKey.from_private_bytes(n.to_bytes(32, "big")).public_key().public_bytes(),
                    3.5, rust_destination + sig_public))
            assert not Stamps.stamp_valid(candidate, 3.5, rust_destination + sig_public)
            body = candidate + sig_public + RNS.Link.signalling_bytes(500, RNS.Link.MODE_DEFAULT)
            RNS.Packet(out, body, packet_type=RNS.Packet.LINKREQUEST).send()
            time.sleep(0.7)
            results["insufficient embedded link work refused"] = "INBOUND" not in peer.lines
            valid = RNS.Link(out)
            results["valid stamped link after refusal reached ACTIVE"] = wait(
                lambda: valid.status == RNS.Link.ACTIVE)
            peer.matching("INBOUND")
            valid.teardown()
            print(json.dumps(results), flush=True)
            assert all(results.values()), results
            return results
        peer.command(f"LINK {destination.hash.hex()}")
        peer.matching(rf"ACTIVE {destination.hash.hex()}")
        results["stock accepted Retinue link"] = wait(lambda: bool(incoming))
        out = RNS.Destination(RNS.Identity.recall(rust_destination), RNS.Destination.OUT,
                              RNS.Destination.SINGLE, "retinue", "network-stamp")
        link = RNS.Link(out)
        results["stock link reached ACTIVE"] = wait(lambda: link.status == RNS.Link.ACTIVE)
        peer.matching("INBOUND")
        link.teardown()
        print(json.dumps(results), flush=True)
        assert all(results.values()), results
        return results
    finally:
        peer.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--example", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--case", choices=CASES)
    parser.add_argument("--only", choices=CASES, action="append")
    parser.add_argument("--mine", type=Path, help=argparse.SUPPRESS)
    if "--mine" in sys.argv:
        mine(Path(sys.argv[sys.argv.index("--mine") + 1]))
        return 0
    args = parser.parse_args()
    if args.case:
        root = Path(os.environ["RETINUE_NETWORK_STAMP_CONFIG"])
        case(args.example.resolve(), args.case, root)
        return 0
    if args.output is None:
        parser.error("--output is required")
    args.output.mkdir(parents=True, exist_ok=False)
    results = {}
    for name in args.only or CASES:
        root = Path(tempfile.mkdtemp(prefix="retinue-network-stamp-"))
        print("START", name, flush=True)
        try:
            with (args.output / f"{name}.log").open("xb") as log:
                proc = subprocess.Popen([sys.executable, "-u", __file__, "--example",
                                         str(args.example.resolve()), "--case", name],
                                        env={**os.environ, "RETINUE_NETWORK_STAMP_CONFIG": str(root)},
                                        stdout=log, stderr=subprocess.STDOUT)
                try:
                    results[name] = proc.wait(timeout=SLOW_CASES.get(name, 60))
                except subprocess.TimeoutExpired:
                    if os.name == "nt":
                        subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    else:
                        proc.kill()
                    proc.wait()
                    results[name] = "timeout"
        finally:
            shutil.rmtree(root, ignore_errors=True)
        (args.output / "result.json").write_text(json.dumps(results, indent=2) + "\n")
        print("RESULT", name, results[name], flush=True)
    return int(any(results.values()))


if __name__ == "__main__":
    raise SystemExit(main())
