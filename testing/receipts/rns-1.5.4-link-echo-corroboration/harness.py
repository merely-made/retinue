"""Orchestration helpers: spawn node.py children, and an HDLC-aware TCP proxy that
records every frame in both directions and can withhold or inject frames.

The orchestrator does not import RNS. Frame decoding below is the independently
written wire layout from Retinue's own reference (header, hops, optional transport
id, destination, context, data), used only to label captured frames.
"""
from __future__ import annotations
import hashlib, json, os, queue, shutil, socket, subprocess, sys, tempfile, threading, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
PY = os.environ.get("RNS_ORACLE_PYTHON",
                    r"C:\Users\mark_\Code\repos\retinue\crates\retinue\oracle\.venv\Scripts\python.exe")

FLAG, ESC, ESC_MASK = 0x7E, 0x7D, 0x20
PTYPES = {0: "DATA", 1: "ANNOUNCE", 2: "LINKREQUEST", 3: "PROOF"}
DTYPES = {0: "SINGLE", 1: "GROUP", 2: "PLAIN", 3: "LINK"}


def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def decode(raw: bytes) -> dict:
    b0 = raw[0]
    header2 = (b0 >> 6) & 1
    d = {"header_type": 2 if header2 else 1, "context_flag": (b0 >> 5) & 1,
         "ptype": PTYPES[b0 & 3], "dtype": DTYPES[(b0 >> 2) & 3], "hops": raw[1]}
    off = 2
    if header2:
        d["transport_id"] = raw[2:18].hex(); off = 18
    d["dest"] = raw[off:off + 16].hex()
    d["context"] = raw[off + 16]
    d["len"] = len(raw)
    hashable = bytes([b0 & 0x0F]) + raw[off:]
    d["packet_hash"] = hashlib.sha256(hashable).hexdigest()
    return d


def frame(raw: bytes) -> bytes:
    out = bytearray([FLAG])
    for b in raw:
        if b in (FLAG, ESC):
            out += bytes([ESC, b ^ ESC_MASK])
        else:
            out.append(b)
    out.append(FLAG)
    return bytes(out)


class Proxy:
    """Listen on `port`, connect to 127.0.0.1:`upstream` per accepted client.

    Direction "up" = client -> upstream server; "down" = server -> client.
    `hold(pred)` withholds the next frame matching pred in a direction.
    """

    def __init__(self, name, upstream, log):
        self.name, self.upstream, self.log = name, upstream, log
        self.port = free_port()
        self.frames = []           # (t, direction, raw)
        self.lock = threading.Lock()
        self.socks = {}
        self.send_locks = {"up": threading.Lock(), "down": threading.Lock()}
        self.holds = []            # [direction, pred, slot(list)]
        self.srv = socket.socket(); self.srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.srv.bind(("127.0.0.1", self.port)); self.srv.listen(1)
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self):
        c, _ = self.srv.accept()
        u = socket.create_connection(("127.0.0.1", self.upstream))
        for s in (c, u):
            s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.socks = {"up": u, "down": c}
        threading.Thread(target=self._pump, args=(c, "up"), daemon=True).start()
        threading.Thread(target=self._pump, args=(u, "down"), daemon=True).start()

    def _pump(self, src, direction):
        buf = bytearray(); in_frame = False; esc = False
        while True:
            try:
                chunk = src.recv(4096)
            except OSError:
                return
            if not chunk:
                try: self.socks["up" if direction == "down" else "down"].shutdown(socket.SHUT_WR)
                except OSError: pass
                return
            for b in chunk:
                if b == FLAG:
                    if in_frame and buf:
                        self._on_frame(direction, bytes(buf))
                    buf = bytearray(); in_frame = True; esc = False
                elif in_frame:
                    if b == ESC: esc = True
                    else:
                        buf.append(b ^ ESC_MASK if esc else b); esc = False

    def _on_frame(self, direction, raw):
        t = time.time()
        held = False
        with self.lock:
            self.frames.append((t, direction, raw))
            for h in list(self.holds):
                if h[0] == direction and h[1](raw):
                    h[2].append(raw); held = True
                    if not h[3]: self.holds.remove(h)
                    break
        info = decode(raw) if len(raw) >= 19 else {"short": len(raw)}
        self.log({"src": "proxy", "proxy": self.name, "t": t, "dir": direction,
                  "held": held, "raw": raw.hex(), **info})
        if not held:
            self._send(direction, raw)

    def _send(self, direction, raw):
        with self.send_locks[direction]:
            self.socks[direction].sendall(frame(raw))

    def hold(self, direction, pred, persistent=False):
        """Withhold the next matching frame (or every one, if persistent)."""
        slot = []
        with self.lock: self.holds.append([direction, pred, slot, persistent])
        return slot

    def inject(self, direction, raw, label):
        t = time.time()
        info = decode(raw)
        self.log({"src": "inject", "proxy": self.name, "t": t, "dir": direction, "label": label,
                  "raw": raw.hex(), **info})
        self._send(direction, raw)

    def find(self, direction, pred, since=0.0):
        with self.lock:
            return [(t, r) for (t, d, r) in self.frames if d == direction and t >= since and pred(r)]


class Node:
    def __init__(self, name, cfg, log, workdir):
        self.name, self.log = name, log
        cfg = dict(cfg); cfg["configdir"] = str(Path(workdir) / name)
        self.events = []
        self.q = queue.Queue()
        self.p = subprocess.Popen([PY, "-u", str(HERE / "node.py"), json.dumps(cfg)],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                  text=True, bufsize=1, cwd=str(HERE))
        threading.Thread(target=self._pump, daemon=True).start()
        self.ready = self.wait("ready", timeout=30)

    def _pump(self):
        for line in self.p.stdout:
            line = line.rstrip()
            try:
                ev = json.loads(line)
            except ValueError:
                self.log({"src": self.name, "stdout": line}); continue
            ev["node"] = self.name
            self.events.append(ev); self.q.put(ev)
            self.log({"src": self.name, **ev})

    def cmd(self, **c):
        self.log({"src": "cmd", "node": self.name, "t": time.time(), **c})
        self.p.stdin.write(json.dumps(c) + "\n"); self.p.stdin.flush()

    def wait(self, ev, timeout=30, pred=lambda e: True):
        # Search already-seen events first, then block on new ones.
        deadline = time.time() + timeout
        seen = getattr(self, "_consumed", 0)
        while True:
            for i in range(seen, len(self.events)):
                e = self.events[i]
                if e["ev"] == ev and pred(e):
                    return e
            seen = len(self.events)
            rem = deadline - time.time()
            if rem <= 0:
                raise TimeoutError(f"{self.name}: no {ev} within {timeout}s")
            try: self.q.get(timeout=min(rem, 0.2))
            except queue.Empty: pass

    def all(self, ev, pred=lambda e: True, since=0.0):
        return [e for e in self.events if e["ev"] == ev and e["t"] >= since and pred(e)]

    def stop(self):
        try:
            self.cmd(cmd="exit"); self.p.wait(timeout=10)
        except Exception:
            self.p.kill()

    def kill(self):
        self.p.kill(); self.p.wait()


class Run:
    """A results directory with a JSONL event log."""

    def __init__(self, tag):
        self.dir = HERE / "results"
        self.dir.mkdir(exist_ok=True)
        self.path = self.dir / f"{tag}.jsonl"
        self.f = open(self.path, "w", encoding="utf-8")
        self.lock = threading.Lock()
        self.work = tempfile.mkdtemp(prefix=f"v1corr-{tag}-")
        self.nodes = []

    def log(self, rec):
        with self.lock:
            self.f.write(json.dumps(rec) + "\n"); self.f.flush()

    def node(self, name, cfg):
        n = Node(name, cfg, self.log, self.work); self.nodes.append(n); return n

    def proxy(self, name, upstream):
        return Proxy(name, upstream, self.log)

    def close(self):
        for n in self.nodes:
            if n.p.poll() is None: n.stop()
        self.f.close()
        shutil.rmtree(self.work, ignore_errors=True)
