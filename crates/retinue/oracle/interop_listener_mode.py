"""Do connections a retinue listener spawns take its mode, as RNS's spawned clients do?

Retinue routes with two listeners, one access-point and one full (TCPInterface.py 587-650).
Stock client A connects to the access-point listener and stock client F to the full one. A
second retinue endpoint then announces through the full listener. RNS relays no announce
onto an access-point interface (Transport.py 1475-1477), so F must learn the path and A
must not: spawned-interface modes and egress mode filtering are checked together.

Run from the oracle/ directory:  ./.venv/bin/python -u interop_listener_mode.py
"""
from __future__ import annotations

import shutil, sys, tempfile, time
from pathlib import Path

from interop_tcp_reconnect import Proc, REPO


def run_client(cfg: str, port: int) -> None:
    import RNS
    Path(cfg).mkdir(parents=True, exist_ok=True)
    (Path(cfg) / "config").write_text(
        "[reticulum]\n  enable_transport = No\n  share_instance = No\n  panic_on_interface_error = No\n"
        "\n[logging]\n  loglevel = 2\n\n[interfaces]\n  [[retinue]]\n    type = TCPClientInterface\n"
        f"    enabled = yes\n    target_host = 127.0.0.1\n    target_port = {port}\n", encoding="utf-8")
    RNS.Reticulum(configdir=cfg)
    print("READY", flush=True)
    for line in sys.stdin:
        cmd = line.split()
        if cmd[:1] == ["PATH"]:
            target, deadline = bytes.fromhex(cmd[1]), time.time() + float(cmd[2])
            while not RNS.Transport.has_path(target) and time.time() < deadline:
                time.sleep(0.1)
            print(f"PATH {RNS.Transport.has_path(target)}", flush=True)


def main() -> int:
    scratch = Path(tempfile.mkdtemp(prefix="retinue-listener-mode-"))
    retinue = Proc("retinue", ["cargo", "run", "--quiet", "--example", "listener_mode"],
                   {"RETINUE_LISTENERS": "full,access_point"}, cwd=REPO)
    clients: list[Proc] = []
    checks: dict[str, bool] = {}
    try:
        full = retinue.expect(r"LISTENING full (\d+)", 300)
        ap = retinue.expect(r"LISTENING access_point (\d+)", 30)
        if not (full and ap):
            return 1
        for label, port, mode in (("F", full[1], "full"), ("A", ap[1], "access_point")):
            client = Proc(label, [sys.executable, "-u", __file__, "--client",
                                  str(scratch / label), port])
            clients.append(client)
            checks[f"{label} spawned on the {mode} listener"] = bool(
                client.expect(r"READY", 60) and retinue.expect(rf"SPAWNED {mode} \d+", 15))
        retinue.send(f"ANNOUNCER {full[1]}")
        relayed = retinue.expect(r"ANNOUNCER_DEST ([0-9a-f]{32})", 15)
        if not relayed:
            return 1
        f, a = clients
        f.send(f"PATH {relayed[1]} 10")
        checks["F (full) has_path for the relayed announce"] = bool(
            (m := f.expect(r"PATH (True|False)", 15)) and m[1] == "True")
        a.send(f"PATH {relayed[1]} 3")
        checks["A (access point) has no path for it"] = bool(
            (m := a.expect(r"PATH (True|False)", 10)) and m[1] == "False")
    finally:
        for p in [retinue, *clients]:
            p.kill()
        shutil.rmtree(scratch, ignore_errors=True)

    print("\n" + "=" * 68)
    for name, ok in checks.items():
        print(f"{'PASS' if ok else 'FAIL'}  {name}")
    print("=" * 68)
    ok = len(checks) == 4 and all(checks.values())
    print(f"LISTENER MODE INTEROP: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    if sys.argv[1:2] == ["--client"]:
        run_client(sys.argv[2], int(sys.argv[3]))
    else:
        raise SystemExit(main())
