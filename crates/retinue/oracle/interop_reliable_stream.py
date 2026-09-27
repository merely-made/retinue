"""Live stock-RNS 1.5.4 Channel/Buffer gate against Retinue's reliable Endpoint.

Build first from crates/retinue/ with the approved target:
  cargo build --locked --offline -j 2 --example reliable_stream_oracle
Run from oracle/: ./.venv/Scripts/python.exe -u interop_reliable_stream.py
The RNS Buffer sends data plus EOF, Retinue reads exact bytes and EOF, then
Retinue sends data plus EOF and RNS checks both. The public Channel.send call is
observed after successful submission to prove a compressed EOF data frame was
actually queued. This does not intercept or replace the packet transport.
"""
from __future__ import annotations

import atexit
import bz2
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import RNS
from RNS.Buffer import StreamDataMessage

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
REQUEST = b"rns-compressed-eof:" + b"Z" * 320
REPLY = b"retinue-reliable-reply:" + b"Q" * 640
COMBINED = b"rns-combined-eof:" + b"Y" * 8192
COMBINED_REPLY = b"retinue-combined-reply"


def main() -> int:
    if RNS.__version__ != "1.5.4":
        raise RuntimeError(f"expected stock RNS 1.5.4, got {RNS.__version__}")
    print(f"RNS {RNS.__version__}", flush=True)
    default_target = r"C:\t\cargo-targets\retinue" if os.name == "nt" else str(REPO.parent.parent / "target")
    target = Path(os.environ.get("CARGO_TARGET_DIR", default_target))
    binary_name = "reliable_stream_oracle.exe" if os.name == "nt" else "reliable_stream_oracle"
    binary = target / "debug" / "examples" / binary_name
    if not binary.is_file():
        raise FileNotFoundError(f"build the reliable_stream_oracle example first: {binary}")
    print(f"Retinue binary: {binary}", flush=True)
    proc = subprocess.Popen(
        [str(binary)],
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    config_dir: Path | None = None

    def cleanup() -> None:
        if proc.poll() is None:
            proc.kill()
        proc.wait(timeout=3)
        if config_dir is not None:
            shutil.rmtree(config_dir, ignore_errors=True)

    # Covers startup/configuration errors too, before the exchange's finally exists.
    atexit.register(cleanup)
    lines: list[str] = []
    lines_lock = threading.Lock()

    def pump() -> None:
        assert proc.stdout is not None
        for raw in proc.stdout:
            line = raw.rstrip()
            with lines_lock:
                lines.append(line)
            print(f"  [retinue] {line}", flush=True)

    threading.Thread(target=pump, daemon=True).start()
    deadline = time.monotonic() + 180
    port = None
    while time.monotonic() < deadline and port is None:
        with lines_lock:
            current = tuple(lines)
        for line in current:
            match = re.fullmatch(r"LISTENING (\d+)", line)
            if match:
                port = int(match.group(1))
                break
        if proc.poll() is not None:
            raise RuntimeError(f"Retinue exited before listening: {proc.returncode}")
        time.sleep(0.1)
    if port is None:
        raise TimeoutError("Retinue did not listen")

    config_dir = Path(os.environ["RETINUE_RELIABLE_CONFIG_DIR"])
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport=No\n  share_instance=No\n"
        "  panic_on_interface_error=No\n\n[logging]\n  loglevel=3\n\n"
        "[interfaces]\n  [[retinue]]\n    type=TCPClientInterface\n"
        "    enabled=yes\n    target_host=127.0.0.1\n"
        f"    target_port={port}\n",
        encoding="utf-8",
    )
    RNS.Reticulum(configdir=str(config_dir))
    initiator_identity = RNS.Identity()
    complete = threading.Event()
    outcome: dict[str, object] = {"sent_frames": []}
    links: list[object] = []  # Keep both live RNS Link objects until verdict.
    writers: list[object] = []  # RNS BufferedWriter finalization must not run mid-link.
    exit_code = 1
    try:
        class Linker:
            aspect_filter = "retinue.reliable-oracle"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if outcome.get("started"):
                    return
                outcome["started"] = True
                remote = RNS.Destination(
                    announced_identity, RNS.Destination.OUT,
                    RNS.Destination.SINGLE, "retinue", "reliable-oracle",
                )
                def start_link(mode: str) -> None:
                    link = RNS.Link(remote)
                    links.append(link)

                    def established(established_link):
                        established_link.identify(initiator_identity)
                        channel = established_link.get_channel()
                        original_send = channel.send

                        def observed_send(message):
                            packed = message.pack()
                            envelope = original_send(message)
                            # Public send succeeded. MessageBase.pack() exposes
                            # the stream header: bit 15 EOF, bit 14 bzip2.
                            if int(getattr(message, "MSGTYPE", -1)) == 0xFF00:
                                header = int.from_bytes(packed[:2], "big")
                                frame = {
                                    "mode": mode,
                                    "eof": bool(header & 0x8000),
                                    "compressed": bool(header & 0x4000),
                                    "data_bytes": len(packed) - 2,
                                }
                                outcome["sent_frames"].append(frame)
                                print(f"  RNS sent stream frame {frame}", flush=True)
                            return envelope

                        channel.send = observed_send

                        def exchange():
                            try:
                                reader = RNS.Buffer.create_reader(0, channel)
                                if mode == "buffer":
                                    writer = RNS.Buffer.create_writer(0, channel)
                                    writers.append(writer)
                                    if writer.write(REQUEST) != len(REQUEST):
                                        raise RuntimeError("RNS Buffer short write")
                                    writer.close()
                                    print("  RNS Buffer writer closed", flush=True)
                                else:
                                    compressed = bz2.compress(COMBINED)
                                    if len(compressed) > StreamDataMessage.MAX_DATA_LEN:
                                        raise RuntimeError("combined frame exceeds Channel MDU")
                                    channel.send(StreamDataMessage(
                                        stream_id=0, data=compressed, eof=True, compressed=True,
                                    ))
                                    print("  RNS public Channel sent compressed EOF frame", flush=True)

                                received = bytearray()
                                deadline = time.monotonic() + 35
                                eof = False
                                while time.monotonic() < deadline:
                                    chunk = reader.read(1024)
                                    if chunk is None:
                                        time.sleep(0.05)
                                        continue
                                    if chunk == b"":
                                        eof = True
                                        break
                                    received.extend(chunk)
                                outcome[f"read_eof_{mode}"] = eof
                                outcome[f"reply_{mode}"] = bytes(received)
                                print(f"  RNS {mode} read {len(received)} reply bytes, EOF={eof}", flush=True)
                                if mode == "buffer" and eof and bytes(received) == REPLY:
                                    start_link("combined")
                                else:
                                    complete.set()
                            except Exception as error:
                                outcome["error"] = repr(error)
                                print(f"  RNS {mode} exchange error: {error!r}", flush=True)
                                complete.set()

                        threading.Thread(target=exchange, daemon=True).start()

                    link.set_link_established_callback(established)

                start_link("buffer")

        RNS.Transport.register_announce_handler(Linker())
        print("waiting for reliable link and Buffer exchange...", flush=True)
        complete.wait(timeout=50)
        try:
            process_code = proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process_code = None
        time.sleep(0.1)
        with lines_lock:
            joined = "\n".join(lines)
        frames = outcome["sent_frames"]
        compressed_eof = any(
            frame["mode"] == "combined" and frame["eof"]
            and frame["compressed"] and frame["data_bytes"] > 0
            for frame in frames
        )
        buffer_out = any(frame["mode"] == "buffer" for frame in frames)
        rns_to_retinue = (
            "RECV_OK 0" in joined and f"READ_EOF 0 {len(REQUEST)}" in joined
            and "RECV_OK 1" in joined and f"READ_EOF 1 {len(COMBINED)}" in joined
        )
        retinue_to_rns = (
            outcome.get("reply_buffer") == REPLY and outcome.get("read_eof_buffer") is True
            and outcome.get("reply_combined") == COMBINED_REPLY
            and outcome.get("read_eof_combined") is True
        )
        print(f"RNS Buffer writer sent a stream frame: {'PASS' if buffer_out else 'FAIL'}")
        print(f"RNS compressed data carrying EOF: {'PASS' if compressed_eof else 'FAIL'}")
        print(f"RNS -> Retinue exact bytes and EOF: {'PASS' if rns_to_retinue else 'FAIL'}")
        print(f"Retinue -> RNS exact bytes and EOF: {'PASS' if retinue_to_rns else 'FAIL'}")
        print(f"Retinue process exited zero: {'PASS' if process_code == 0 else 'FAIL'} ({process_code})")
        if outcome.get("error"):
            print(f"exchange error: {outcome['error']}")
        success = (
            buffer_out and compressed_eof and rns_to_retinue and retinue_to_rns
            and "DONE" in joined and process_code == 0
        )
        print(f"RELIABLE STREAM INTEROP: {'PASS' if success else 'FAIL'}")
        exit_code = 0 if success else 1
        return exit_code
    finally:
        try:
            RNS.exit(exit_code)
        finally:
            cleanup()
            atexit.unregister(cleanup)


def supervise() -> int:
    # RNS.exit can hard-exit before Python atexit handlers run. The parent owns
    # the disposable RNS config and removes it after the peer process exits.
    config_dir = Path(tempfile.mkdtemp(prefix="retinue-reliable-oracle-"))
    env = os.environ.copy()
    env["RETINUE_RELIABLE_CONFIG_DIR"] = str(config_dir)
    peer = subprocess.Popen([sys.executable, "-u", __file__, "--peer"], env=env)
    try:
        return peer.wait(timeout=120)
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(peer.pid), "/T", "/F"], check=False)
        else:
            peer.kill()
        peer.wait(timeout=5)
        print("RELIABLE STREAM INTEROP: FAIL (peer timed out)", flush=True)
        return 1
    finally:
        shutil.rmtree(config_dir, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main() if sys.argv[1:] == ["--peer"] else supervise())
