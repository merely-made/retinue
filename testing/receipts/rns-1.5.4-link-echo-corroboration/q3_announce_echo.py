"""Q3: what does a stock RNS 1.5.4 node do when a relay's rebroadcast of its OWN
announce comes back to it?

Topology:   C -> P3 -> A(transport) -> P1 -> R(transport) -> P2 -> B
A is transport-enabled with two interfaces, so a rebroadcast by A would be visible
on P1 (toward R) and on P3 (toward C). B's announce is the positive control: A
must record B as a known destination, learn a path, fire its announce handler and
rebroadcast it to C. P2 captures R's retransmission of A's own announce toward B;
P1 injects those exact bytes into A, as a shared medium would deliver them.

With --suppress-natural, P1 withholds every R->A copy of A's announce, so A's
state can be read once with no echo at all and again after one injected echo.

Usage: python q3_announce_echo.py [--tag name] [--suppress-natural]
"""
from __future__ import annotations
import argparse, json, time
from harness import HERE, Run, decode, free_port


def ann_for(dest):
    def pred(r):
        d = decode(r)
        return d["ptype"] == "ANNOUNCE" and d["dest"] == dest
    return pred


def snapshot(node, dest, label):
    t = time.time()
    node.cmd(cmd="state", dest=dest)
    e = node.wait("state", timeout=10, pred=lambda e: e["t"] >= t and e["dest"] == dest)
    return {k: e[k] for k in ("has_path", "hops_to", "recall_app_data", "in_known_destinations")}


def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--tag", default="q3_announce_echo")
    ap.add_argument("--suppress-natural", action="store_true"); a = ap.parse_args()
    run = Run(a.tag)
    res = {"case": a.tag}
    try:
        pB, pR, pA = free_port(), free_port(), free_port()
        B = run.node("B", {"transport": False, "interfaces": [{"type": "server", "port": pB}]})
        P2 = run.proxy("P2_R_B", pB)
        R = run.node("R", {"transport": True, "interfaces": [{"type": "server", "port": pR},
                                                            {"type": "client", "port": P2.port}]})
        P1 = run.proxy("P1_A_R", pR)
        A = run.node("A", {"transport": True, "interfaces": [{"type": "client", "port": P1.port},
                                                            {"type": "server", "port": pA}]})
        P3 = run.proxy("P3_C_A", pA)
        C = run.node("C", {"transport": False, "interfaces": [{"type": "client", "port": P3.port}]})
        time.sleep(3)
        A.cmd(cmd="handler"); A.wait("handler_registered")
        A.cmd(cmd="dest", name="self", accept_links=True); own = A.wait("dest")["hash"]
        B.cmd(cmd="dest", name="peerb", accept_links=True); peer = B.wait("dest")["hash"]
        res["own_dest"], res["peer_dest"] = own, peer
        res["A_own_state_initial"] = snapshot(A, own, "initial")

        # Positive control: a genuine peer announce through R.
        t = time.time(); B.cmd(cmd="announce", name="peerb", app_data="B-app"); time.sleep(6)
        res["ctl_A_handler_events_for_peer"] = len(A.all("announce_handler", lambda e: e["dest"] == peer, since=t))
        res["ctl_A_peer_state"] = snapshot(A, peer, "peer")
        res["ctl_A_rebroadcast_peer_to_C_frames"] = len(P3.find("down", ann_for(peer), since=t))
        res["ctl_C_peer_state"] = snapshot(C, peer, "C-peer")

        # A announces itself; record what each hop does with it.
        blocked = P1.hold("down", ann_for(own), persistent=True) if a.suppress_natural else None
        res["suppress_natural_echo"] = a.suppress_natural
        t = time.time(); A.cmd(cmd="announce", name="self", app_data="A-app"); time.sleep(6)
        res["A_own_announce_frames_to_R"] = len(P1.find("up", ann_for(own), since=t))
        res["A_own_announce_frames_to_C"] = len(P3.find("down", ann_for(own), since=t))
        relayed = P2.find("up", ann_for(own), since=t)
        res["R_rebroadcast_frames_toward_B"] = len(relayed)
        res["R_natural_echo_frames_back_to_A"] = len(P1.find("down", ann_for(own), since=t))
        res["A_own_state_after_own_announce"] = snapshot(A, own, "after-announce")
        if blocked is not None:
            res["natural_echo_frames_withheld"] = len(blocked)
        res["ctl_B_learned_A"] = snapshot(B, own, "B-own")
        if relayed:
            d = decode(relayed[0][1])
            res["R_rebroadcast_header"] = {k: d.get(k) for k in ("header_type", "hops", "transport_id", "context")}
        res["R_transport_id"] = R.ready["transport_id"]

        # Inject R's rebroadcast of A's announce back into A.
        t = time.time(); P1.inject("down", relayed[0][1], "R's rebroadcast of A's own announce"); time.sleep(10)
        res["inj_A_handler_events_for_own"] = len(A.all("announce_handler", lambda e: e["dest"] == own, since=t))
        res["inj_A_own_announce_frames_to_R_after"] = len(P1.find("up", ann_for(own), since=t))
        res["inj_A_own_announce_frames_to_C_after"] = len(P3.find("down", ann_for(own), since=t))
        res["inj_A_own_state_after_echo"] = snapshot(A, own, "after-echo")

        # Second injection: A's announce exactly as A emitted it.
        orig = P1.find("up", ann_for(own))
        t = time.time(); P1.inject("down", orig[0][1], "A's own announce verbatim"); time.sleep(10)
        res["inj2_A_handler_events_for_own"] = len(A.all("announce_handler", lambda e: e["dest"] == own, since=t))
        res["inj2_A_own_announce_frames_out_after"] = (len(P1.find("up", ann_for(own), since=t)) +
                                                      len(P3.find("down", ann_for(own), since=t)))
        res["inj2_A_own_state"] = snapshot(A, own, "after-echo-2")

        run.log({"src": "result", **res})
        print(json.dumps(res, indent=1), flush=True)
        with open(HERE / "results" / f"{a.tag}_summary.json", "w", encoding="utf-8") as f:
            json.dump(res, f, indent=1)
    finally:
        run.close()


if __name__ == "__main__":
    main()
