"""Does a retinue relay's announce retry stop once RNS passes the announce on?

Topology:  retinue-responder  <->  retinue transport node  <->  RNS transport node

The responder announces; the retinue node relays each announce after a jitter and holds it
for one retry 5.5 s later (Transport.py 765-829). The RNS node, being a transport, rebroadcasts
what it learns back over the same TCP link with one more hop, which the retinue node must take
as "passed on" and cancel its retry (Transport.py 2196-2201). So RNS hears each relayed
announce from the retinue node exactly once.

Run from the oracle/ directory:  ./.venv/bin/python -u interop_rebroadcast.py
"""
from __future__ import annotations
import atexit, os, shutil, subprocess, sys, tempfile, threading, time
from collections import Counter
from pathlib import Path
import RNS

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
HUB_PORT = 46031
heard: Counter = Counter()
hops_seen: set = set()
lock = threading.Lock()


def spawn(example, env_extra, sink, label):
    env = dict(os.environ, **env_extra)
    p = subprocess.Popen(["cargo", "run", "--quiet", "--example", example],
        cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1, env=env)
    def pump():
        for line in p.stdout:
            sink.append(line.rstrip()); print(f"  [{label}] {line.rstrip()}")
    threading.Thread(target=pump, daemon=True).start()
    return p


def main() -> int:
    print(f"RNS {RNS.__version__}\n")
    sink = []
    hub = spawn("transport_node", {"RETINUE_PORT": str(HUB_PORT)}, sink, "hub")
    deadline = time.time() + 120
    while time.time() < deadline and not any("TRANSPORT_NODE_UP" in l for l in sink):
        if hub.poll() is not None:
            print("hub exited"); return 1
        time.sleep(0.3)
    hub_id = bytes.fromhex(next(l for l in sink if "TRANSPORT_NODE_UP" in l).split()[1])

    cfg = Path(tempfile.mkdtemp(prefix="retinue-rnsrelay-"))
    atexit.register(shutil.rmtree, cfg, ignore_errors=True)
    (cfg / "config").write_text(
        "[reticulum]\n  enable_transport = Yes\n  share_instance = No\n  panic_on_interface_error = No\n"
        f"\n[logging]\n  loglevel = 4\n\n[interfaces]\n  [[hub]]\n    type = TCPClientInterface\n    enabled = yes\n"
        f"    target_host = 127.0.0.1\n    target_port = {HUB_PORT}\n", encoding="utf-8")

    original = RNS.Transport.inbound
    def counting(raw, *args, **kwargs):
        packet = RNS.Packet(None, raw)
        if packet.unpack() and packet.packet_type == RNS.Packet.ANNOUNCE \
                and packet.transport_id == hub_id:
            with lock:
                heard[packet.get_hash()] += 1
                hops_seen.add(packet.hops)
        return original(raw, *args, **kwargs)
    RNS.Transport.inbound = counting
    RNS.Reticulum(configdir=str(cfg))
    time.sleep(1.0)

    resp = spawn("link_peer", {"RETINUE_ROLE": "responder", "RETINUE_ADDR": f"127.0.0.1:{HUB_PORT}"},
                 sink, "resp")
    # Six announces over 2.4 s, then past every retry the hub could still owe (5.5 s + jitter).
    first = time.time() + 30
    while time.time() < first and not heard:
        time.sleep(0.2)
    time.sleep(12)
    for p in (resp, hub):
        try: p.kill()
        except Exception: pass

    with lock:
        counts = sorted(heard.values())
    relayed = len(counts) > 0
    once = relayed and all(c == 1 for c in counts)
    print("\n" + "=" * 68)
    print(f"distinct relayed announces heard from the retinue node: {len(counts)}")
    print(f"times each was heard:                                   {counts}")
    print(f"hops on the wire:                                       {sorted(hops_seen)}")
    print("=" * 68)
    ok = once and hops_seen == {1}
    print(f"R39 RETINUE-RETRY-CANCELLED-BY-RNS: {'PASS' if ok else 'FAIL'}")
    exit_code = 0 if ok else 1
    RNS.exit(exit_code)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
