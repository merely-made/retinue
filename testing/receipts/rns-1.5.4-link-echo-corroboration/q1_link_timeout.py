"""Q1: how long does a stock RNS 1.5.4 initiator wait for an unanswered link request?

Chain:  A(initiator) -> proxy P -> R1 -> ... -> Rn -> B(responder)
All RNS instances are separate processes on localhost TCP; relays have transport
enabled. B's destination refuses links (public `accepts_links(False)`), so the
request is carried end to end and never proved. P records every frame A sends and
receives, so retries and any proof are visible on the wire.

Usage: python q1_link_timeout.py [--reps N] [--quick]
"""
from __future__ import annotations
import argparse, json, time
from harness import Run, decode, free_port

LINKREQUEST = 2


def build(run, relays, accept_links, bitrate=None):
    pB = free_port()
    B = run.node("B", {"transport": False, "interfaces": [{"type": "server", "port": pB}]})
    nxt = pB
    rel = []
    for k in range(relays, 0, -1):
        pR = free_port()
        rel.append(run.node(f"R{k}", {"transport": True,
                                      "interfaces": [{"type": "server", "port": pR}, {"type": "client", "port": nxt}]}))
        nxt = pR
    P = run.proxy("P_A", nxt)
    A = run.node("A", {"transport": False,
                       "interfaces": [{"type": "client", "port": P.port, "bitrate": bitrate}]})
    time.sleep(3)  # let TCP interfaces come up
    B.cmd(cmd="dest", name="resp", accept_links=accept_links)
    dest = B.wait("dest")["hash"]
    B.cmd(cmd="announce", name="resp")
    A.cmd(cmd="wait_path", dest=dest, hops=relays + 1, timeout=60)
    path = A.wait("path", timeout=70)
    return A, B, rel, P, dest, path


def unanswered(tag, relays, bitrate=None):
    run = Run(tag)
    try:
        A, B, rel, P, dest, path = build(run, relays, accept_links=False, bitrate=bitrate)
        A.cmd(cmd="link", dest=dest, name="resp")
        req = A.wait("link_request", timeout=10)
        closed = A.wait("link_closed", timeout=req["establishment_timeout"] + 30)
        meta = A.wait("link_closed_meta", timeout=5)
        time.sleep(1)
        lr_frames = P.find("up", lambda r: (r[0] & 3) == LINKREQUEST)
        proofs = P.find("down", lambda r: (r[0] & 3) == 3)
        res = {"case": tag, "relays": relays, "initiator_bitrate": bitrate,
               "hops_to": req["hops_to"], "path_hops": path["hops"],
               "rns_establishment_timeout_s": req["establishment_timeout"],
               "observed_wait_s": round(closed["t"] - req["t0"], 3),
               "teardown_reason": closed["teardown_reason"], "status": closed["status"],
               "activated_at": closed["activated_at"], "established_cb_fired": meta["established_cb_fired"],
               "linkrequest_frames_on_wire": len(lr_frames),
               "linkrequest_frame_times_rel_s": [round(t - req["t0"], 3) for t, _ in lr_frames],
               "proof_frames_to_A": len(proofs)}
        run.log({"src": "result", **res})
        return res
    finally:
        run.close()


def answered_controls(tag, relays):
    """Positive control and the two established-then-closed outcomes."""
    run = Run(tag)
    out = {"case": tag, "relays": relays}
    try:
        A, B, rel, P, dest, path = build(run, relays, accept_links=True)
        # (a) establishes, then responder closes the link
        A.cmd(cmd="link", dest=dest, name="resp")
        req = A.wait("link_request", timeout=10)
        est = A.wait("link_established", timeout=req["establishment_timeout"] + 5)
        out["established_after_s"] = round(est["t"] - req["t0"], 3)
        B.wait("link_established", timeout=10)
        B.cmd(cmd="teardown")
        c = A.wait("link_closed", timeout=20)
        out["remote_close"] = {"teardown_reason": c["teardown_reason"], "status": c["status"],
                               "activated_at_set": c["activated_at"] is not None}
        # (b) establishes, then responder vanishes silently -> stale timeout
        t_mark = time.time()
        A.cmd(cmd="link", dest=dest, name="resp")
        req2 = A.wait("link_request", timeout=10, pred=lambda e: e["t"] >= t_mark)
        A.wait("link_established", timeout=req2["establishment_timeout"] + 5,
               pred=lambda e: e["link_id"] == req2["link_id"])
        time.sleep(1)
        t_kill = time.time(); B.kill()
        c2 = A.wait("link_closed", timeout=120, pred=lambda e: e["link_id"] == req2["link_id"])
        out["silent_loss"] = {"teardown_reason": c2["teardown_reason"], "status": c2["status"],
                              "activated_at_set": c2["activated_at"] is not None,
                              "closed_after_kill_s": round(c2["t"] - t_kill, 3)}
        run.log({"src": "result", **out})
        return out
    finally:
        run.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reps", type=int, default=2)
    ap.add_argument("--quick", action="store_true")
    ap.add_argument("--tail-only", action="store_true", help="skip the plain unanswered matrix")
    a = ap.parse_args()
    results = []
    if a.tail_only: a.reps = 0
    relay_counts = [0] if a.quick else [0, 1, 2, 3]
    for rep in range(a.reps):
        for n in relay_counts:
            r = unanswered(f"q1_unanswered_r{n}_rep{rep}", n); print(json.dumps(r), flush=True); results.append(r)
    if not a.quick:
        # 62500 bps is the lowest configured bitrate RNS 1.5.4 accepts on a TCP
        # interface: below it optimise_mtu() leaves HW_MTU None and every inbound
        # frame raises (observed with 1000 bps; see README).
        for n in (0, 2):
            r = unanswered(f"q1_unanswered_r{n}_bitrate62500", n, bitrate=62500); print(json.dumps(r), flush=True); results.append(r)
        for n in (1, 3):
            r = answered_controls(f"q1_controls_r{n}", n); print(json.dumps(r), flush=True); results.append(r)
    with open("results/q1_tail_summary.json" if a.tail_only else "results/q1_summary.json", "w", encoding="utf-8") as f:
        json.dump(results, f, indent=1)


if __name__ == "__main__":
    main()
