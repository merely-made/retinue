"""One stock RNS instance, driven over stdin with JSON-line commands.

Uses only the public RNS API. Emits JSON-line events on stdout, each stamped with
wall-clock time.time(). Argv[1] is a JSON object:
  {"configdir": str, "transport": bool, "interfaces": [ {"type": "server"|"client",
   "port": int, "bitrate": int|null}, ... ], "loglevel": int}
"""
from __future__ import annotations
import json, os, sys, threading, time

import RNS
from RNS.Channel import MessageBase

APP = "v1corr"
LOCK = threading.Lock()


def emit(ev, **kw):
    kw = {"ev": ev, "t": time.time(), **kw}
    with LOCK:
        sys.stdout.write(json.dumps(kw) + "\n")
        sys.stdout.flush()


class ProbeMessage(MessageBase):
    MSGTYPE = 0xABCD

    def __init__(self, payload: bytes = b""):
        self.payload = payload

    def pack(self) -> bytes:
        return self.payload

    def unpack(self, raw: bytes) -> None:
        self.payload = raw


def write_config(cfg):
    d = cfg["configdir"]
    os.makedirs(d, exist_ok=True)
    lines = ["[reticulum]",
             f"  enable_transport = {'Yes' if cfg['transport'] else 'No'}",
             "  share_instance = No", "  panic_on_interface_error = No", "",
             "[logging]", f"  loglevel = {cfg.get('loglevel', 2)}", "", "[interfaces]"]
    for i, itf in enumerate(cfg["interfaces"]):
        lines.append(f"  [[if{i}_{itf['type']}]]")
        if itf["type"] == "server":
            lines += ["    type = TCPServerInterface", "    enabled = yes",
                      "    listen_ip = 127.0.0.1", f"    listen_port = {itf['port']}"]
        else:
            lines += ["    type = TCPClientInterface", "    enabled = yes",
                      "    target_host = 127.0.0.1", f"    target_port = {itf['port']}"]
        if itf.get("bitrate"):
            lines.append(f"    bitrate = {itf['bitrate']}")
    with open(os.path.join(d, "config"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


class Node:
    def __init__(self, cfg):
        write_config(cfg)
        self.rns = RNS.Reticulum(configdir=cfg["configdir"])
        self.identity = RNS.Identity()
        self.dests = {}          # name -> IN destination
        self.link = None         # most recent link (either role)
        self.links = []
        emit("ready", rns_version=RNS.__version__,
             transport_id=RNS.Transport.identity.hash.hex(),
             transport=bool(cfg["transport"]))

    # ---- link plumbing -------------------------------------------------
    def _attach(self, link, role):
        self.link = link
        self.links.append(link)
        link.set_packet_callback(lambda data, pkt: emit(
            "link_data", role=role, link_id=link.link_id.hex(), data=data.decode("utf-8", "replace"),
            hops=pkt.hops, packet_hash=pkt.packet_hash.hex()))
        ch = link.get_channel()
        ch.register_message_type(ProbeMessage)

        def on_msg(msg):
            emit("channel_msg", role=role, link_id=link.link_id.hex(),
                 data=msg.payload.decode("utf-8", "replace"), sequence=getattr(msg, "sequence", None))
            return True
        ch.add_message_handler(on_msg)

    def _link_closed(self, link, role):
        emit("link_closed", role=role, link_id=link.link_id.hex(), status=link.status,
             teardown_reason=link.teardown_reason,
             activated_at=link.activated_at)

    # ---- commands ------------------------------------------------------
    def cmd_dest(self, c):
        d = RNS.Destination(self.identity, RNS.Destination.IN, RNS.Destination.SINGLE, APP, c["name"])
        d.accepts_links(bool(c.get("accept_links", True)))

        def established(link):
            self._attach(link, "responder")
            link.set_link_closed_callback(lambda l: self._link_closed(l, "responder"))
            emit("link_established", role="responder", link_id=link.link_id.hex())
        d.set_link_established_callback(established)
        d.set_packet_callback(lambda data, pkt: emit("dest_data", data=data.decode("utf-8", "replace")))
        self.dests[c["name"]] = d
        emit("dest", name=c["name"], hash=d.hash.hex())

    def cmd_announce(self, c):
        d = self.dests[c["name"]]
        app_data = c.get("app_data")
        d.announce(app_data=app_data.encode() if app_data else None)
        emit("announced", name=c["name"], hash=d.hash.hex())

    def cmd_handler(self, c):
        class H:
            aspect_filter = None

            def received_announce(self, destination_hash, announced_identity, app_data):
                emit("announce_handler", dest=destination_hash.hex(),
                     app_data=app_data.decode("utf-8", "replace") if app_data else None)
        RNS.Transport.register_announce_handler(H())
        emit("handler_registered")

    def cmd_wait_path(self, c):
        h = bytes.fromhex(c["dest"])
        deadline = time.time() + c.get("timeout", 30)
        want = c.get("hops")
        last_req = 0
        while time.time() < deadline:
            if RNS.Transport.has_path(h) and (want is None or RNS.Transport.hops_to(h) == want):
                emit("path", dest=c["dest"], hops=RNS.Transport.hops_to(h)); return
            if time.time() - last_req > 5:
                RNS.Transport.request_path(h); last_req = time.time()
            time.sleep(0.1)
        emit("path_timeout", dest=c["dest"], has_path=RNS.Transport.has_path(h),
             hops=RNS.Transport.hops_to(h) if RNS.Transport.has_path(h) else None)

    def cmd_link(self, c):
        h = bytes.fromhex(c["dest"])
        ident = RNS.Identity.recall(h)
        d = RNS.Destination(ident, RNS.Destination.OUT, RNS.Destination.SINGLE, APP, c["name"])
        state = {"established": False}

        def established(link):
            state["established"] = True
            self._attach(link, "initiator")
            emit("link_established", role="initiator", link_id=link.link_id.hex(), rtt=link.rtt)

        hops = RNS.Transport.hops_to(h)
        t0 = time.time()
        link = RNS.Link(d, established_callback=established,
                        closed_callback=lambda l: (self._link_closed(l, "initiator"),
                                                   emit("link_closed_meta", established_cb_fired=state["established"])))
        emit("link_request", t0=t0, link_id=link.link_id.hex(), hops_to=hops,
             establishment_timeout=link.establishment_timeout, request_time=link.request_time)

    def cmd_send(self, c):
        p = RNS.Packet(self.link, c["data"].encode())
        p.send()
        emit("sent", data=c["data"], packet_hash=p.packet_hash.hex())

    def cmd_chan_send(self, c):
        ch = self.link.get_channel()
        env = ch.send(ProbeMessage(c["data"].encode()))
        emit("chan_sent", data=c["data"], sequence=getattr(env, "sequence", None))

    def cmd_teardown(self, c):
        self.link.teardown()
        emit("teardown_called")

    def cmd_state(self, c):
        h = bytes.fromhex(c["dest"])
        has = RNS.Transport.has_path(h)
        emit("state", dest=c["dest"], has_path=has,
             hops_to=RNS.Transport.hops_to(h) if has else None,
             recall_app_data=(lambda a: a.decode("utf-8", "replace") if a else None)(RNS.Identity.recall_app_data(h)),
             in_known_destinations=h in RNS.Identity.known_destinations)

    def run(self):
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            c = json.loads(line)
            if c["cmd"] == "exit":
                emit("exiting")
                RNS.exit()
                return
            try:
                getattr(self, "cmd_" + c["cmd"])(c)
            except Exception as e:
                emit("error", cmd=c["cmd"], error=repr(e))


if __name__ == "__main__":
    Node(json.loads(sys.argv[1])).run()
