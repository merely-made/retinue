"""Q2: does a stock RNS 1.5.4 node deliver its own link data when a relay's
retransmission of that packet comes back to it? Does it deliver a duplicate?

Topology:  A(initiator) -> P1 -> R(transport) -> P2 -> B(responder)
A's link to B is carried by R. P2 captures R's retransmission of A's packet toward B
(the exact bytes a relay put on the medium) and P1 re-injects it into A, as a shared
medium would deliver it. P1 can also withhold one of B's frames and inject it later,
which proves an injected frame can reach A's application at all.

Usage: python q2_link_echo.py [--tag name]
"""
from __future__ import annotations
import argparse, json, time
from harness import HERE, Run, decode, free_port

DATA, LINK_DT = 0, 3
CTX_NONE, CTX_CHANNEL = 0x00, 0x0E


def is_link_data(ctx):
    def pred(r):
        d = decode(r)
        return d["ptype"] == "DATA" and d["dtype"] == "LINK" and d["context"] == ctx
    return pred


def with_hops(raw, hops):
    return raw[:1] + bytes([hops]) + raw[2:]


def got(node, ev, data, since):
    return [e for e in node.all(ev, since=since) if e.get("data") == data]


def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--tag", default="q2_link_echo"); a = ap.parse_args()
    run = Run(a.tag)
    res = {"case": a.tag}
    try:
        pB = free_port(); pR = free_port()
        B = run.node("B", {"transport": False, "interfaces": [{"type": "server", "port": pB}]})
        P2 = run.proxy("P2_R_B", pB)
        R = run.node("R", {"transport": True, "interfaces": [{"type": "server", "port": pR},
                                                            {"type": "client", "port": P2.port}]})
        P1 = run.proxy("P1_A_R", pR)
        A = run.node("A", {"transport": False, "interfaces": [{"type": "client", "port": P1.port}]})
        time.sleep(3)
        B.cmd(cmd="dest", name="resp", accept_links=True)
        dest = B.wait("dest")["hash"]
        B.cmd(cmd="announce", name="resp")
        A.cmd(cmd="wait_path", dest=dest, hops=2, timeout=60)
        res["path_hops"] = A.wait("path", timeout=70)["hops"]
        A.cmd(cmd="link", dest=dest, name="resp")
        A.wait("link_established", timeout=30); B.wait("link_established", timeout=10)
        time.sleep(1)

        # C0 positive control: genuine data both ways over the relayed link.
        t = time.time()
        A.cmd(cmd="send", data="A-1"); B.cmd(cmd="send", data="B-1"); time.sleep(2)
        res["C0_B_received_A1"] = len(got(B, "link_data", "A-1", t))
        res["C0_A_received_B1"] = len(got(A, "link_data", "B-1", t))

        # C1 instrument control: withhold B's next data frame at P1, then inject it.
        slot = P1.hold("down", is_link_data(CTX_NONE))
        t = time.time(); B.cmd(cmd="send", data="B-2"); time.sleep(2)
        res["C1_frame_withheld"] = len(slot) == 1
        res["C1_A_received_B2_while_withheld"] = len(got(A, "link_data", "B-2", t))
        held = slot[0]
        t = time.time(); P1.inject("down", held, "C1 withheld B-2, first delivery"); time.sleep(2)
        res["C1_A_received_B2_after_inject"] = len(got(A, "link_data", "B-2", t))

        # C2 duplicate: the same frame again, then with a different hop count.
        t = time.time(); P1.inject("down", held, "C2 duplicate B-2 identical"); time.sleep(2)
        res["C2_A_received_B2_identical_dup"] = len(got(A, "link_data", "B-2", t))
        t = time.time(); P1.inject("down", with_hops(held, held[1] + 1), "C2 duplicate B-2 hops+1"); time.sleep(2)
        res["C2_A_received_B2_dup_hops_plus1"] = len(got(A, "link_data", "B-2", t))

        # C3 own echo: R's retransmission of A's packet (captured toward B) back into A.
        t = time.time(); A.cmd(cmd="send", data="A-2")
        sent = A.wait("sent", pred=lambda e: e["data"] == "A-2")
        time.sleep(2)
        relayed = P2.find("up", lambda r: decode(r)["packet_hash"] == sent["packet_hash"], since=t)
        original = P1.find("up", lambda r: decode(r)["packet_hash"] == sent["packet_hash"], since=t)
        res["C3_B_received_A2"] = len(got(B, "link_data", "A-2", t))
        res["C3_relay_retransmission_captured"] = len(relayed)
        res["C3_original_captured"] = len(original)
        res["C3_original_hops"] = original[0][1][1] if original else None
        res["C3_relayed_hops"] = relayed[0][1][1] if relayed else None
        res["C3_hash_equal_original_vs_relayed"] = bool(relayed and original and
                                                        decode(relayed[0][1])["packet_hash"] == decode(original[0][1])["packet_hash"])
        t = time.time(); P1.inject("down", relayed[0][1], "C3 relay retransmission of A-2"); time.sleep(2)
        res["C3_A_received_own_A2_relayed_echo"] = len(got(A, "link_data", "A-2", t))
        t = time.time(); P1.inject("down", original[0][1], "C3 A-2 as A sent it"); time.sleep(2)
        res["C3_A_received_own_A2_verbatim_echo"] = len(got(A, "link_data", "A-2", t))
        # Link still healthy after the echoes?
        t = time.time(); B.cmd(cmd="send", data="B-3"); time.sleep(2)
        res["C3_A_received_B3_after_echoes"] = len(got(A, "link_data", "B-3", t))

        # C4 channel context (bypasses RNS's packet-hash filter per source review).
        # A sends sequence 0 before it has received anything from B's channel.
        t = time.time(); A.cmd(cmd="chan_send", data="A-c0"); time.sleep(2)
        res["C4_B_received_Ac0"] = len(got(B, "channel_msg", "A-c0", t))
        crel = P2.find("up", is_link_data(CTX_CHANNEL), since=t)
        res["C4_relay_channel_frames_captured"] = len(crel)
        t = time.time(); P1.inject("down", crel[0][1], "C4 relay retransmission of A-c0"); time.sleep(2)
        res["C4_A_received_own_Ac0"] = len(got(A, "channel_msg", "A-c0", t))
        res["C4_A_proofs_sent_after_echo"] = len(P1.find("up", lambda r: decode(r)["ptype"] == "PROOF", since=t))
        # Positive control for the channel instrument: genuine B messages.
        t = time.time(); B.cmd(cmd="chan_send", data="B-c0"); time.sleep(2)
        res["C4_A_received_Bc0"] = len(got(A, "channel_msg", "B-c0", t))
        t = time.time(); B.cmd(cmd="chan_send", data="B-c1"); time.sleep(2)
        res["C4_A_received_Bc1"] = len(got(A, "channel_msg", "B-c1", t))
        # Second identical channel echo
        t = time.time(); P1.inject("down", crel[0][1], "C4 second identical channel echo"); time.sleep(2)
        res["C4_A_received_own_Ac0_second"] = len(got(A, "channel_msg", "A-c0", t))

        run.log({"src": "result", **res})
        print(json.dumps(res, indent=1), flush=True)
        with open(HERE / "results" / f"{a.tag}_summary.json", "w", encoding="utf-8") as f:
            json.dump(res, f, indent=1)
    finally:
        run.close()


if __name__ == "__main__":
    main()
