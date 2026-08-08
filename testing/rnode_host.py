"""A minimal RNode host, so we can see exactly what our board hands up.

Speaks just enough of the RNode host protocol to configure the radio, turn it on, and then
print every device-to-host frame with its command byte, length and hex. Nothing here
interprets Reticulum; the point is to see the bytes before anything can silently discard
them.

Opens the port with DTR and RTS deasserted, because asserting either resets an ESP32-S3 and
wipes the very state we came to read.

  python rnode_host.py COM6 [seconds]
"""
import sys, time

sys.path.insert(0, r"C:\Users\mark_\Code\repos\retinue\crates\retinue\oracle\.venv\Lib\site-packages")
import serial

PORT = sys.argv[1] if len(sys.argv) > 1 else "COM6"
SECONDS = int(sys.argv[2]) if len(sys.argv) > 2 else 90

FEND, FESC, TFEND, TFESC = 0xC0, 0xDB, 0xDC, 0xDD

# From crates/radio-hand/src/rnode.rs
DATA, FREQUENCY, BANDWIDTH, TXPOWER, SF, CR = 0x00, 0x01, 0x02, 0x03, 0x04, 0x05
RADIO_STATE, DETECT, STAT_RSSI, STAT_SNR = 0x06, 0x08, 0x23, 0x24
PLATFORM, MCU, FW_VERSION, ERROR = 0x48, 0x49, 0x50, 0x90
DETECT_REQ = 0x73
RSSI_OFFSET = 157

NAMES = {
    DATA: "DATA", FREQUENCY: "FREQUENCY", BANDWIDTH: "BANDWIDTH", TXPOWER: "TXPOWER",
    SF: "SF", CR: "CR", RADIO_STATE: "RADIO_STATE", DETECT: "DETECT",
    STAT_RSSI: "STAT_RSSI", STAT_SNR: "STAT_SNR", PLATFORM: "PLATFORM", MCU: "MCU",
    FW_VERSION: "FW_VERSION", ERROR: "ERROR",
}

# Merely's trunk profile, the one the phone is matched to.
FREQ_HZ, BW_HZ, TXP_DBM, SPREADING, CODING = 906_875_000, 250_000, 17, 8, 5


def escape(payload):
    out = bytearray()
    for byte in payload:
        if byte == FEND:
            out += bytes([FESC, TFEND])
        elif byte == FESC:
            out += bytes([FESC, TFESC])
        else:
            out.append(byte)
    return bytes(out)


def frame(command, payload):
    return bytes([FEND, command]) + escape(payload) + bytes([FEND])


port = serial.Serial()
port.port = PORT
port.baudrate = 115200
port.timeout = 0.2
port.dtr = False   # set before open: asserting either resets an ESP32-S3
port.rts = False
port.open()
time.sleep(1.5)
port.reset_input_buffer()

# Text probes first, while the deframer is still idle. `air` is the executive's own account
# of the radio (what armed, what actually arrived, what was damaged); `rnode` is the
# channel's (whether the host turned the radio on, and what it refused or dropped). Asked
# before any KISS frame, because the channel only reads text at a frame boundary.
for probe in (b"air\n", b"rnode\n"):
    port.write(probe)
    port.flush()
    time.sleep(1.2)
    answer = port.read(1024)
    # Printable ASCII only: the console is cp1252 and the board may still be emitting
    # binary from a previous host session.
    readable = "".join(chr(b) if 32 <= b < 127 else "." for b in answer).strip()
    print("probe %-6s %s" % (probe.strip().decode(), readable if readable else "(no answer)"))

print("configuring %s: %d Hz, BW %d, SF%d, CR4/%d, %d dBm"
      % (PORT, FREQ_HZ, BW_HZ, SPREADING, CODING, TXP_DBM))
port.write(frame(DETECT, bytes([DETECT_REQ])))
port.write(frame(FREQUENCY, FREQ_HZ.to_bytes(4, "big")))
port.write(frame(BANDWIDTH, BW_HZ.to_bytes(4, "big")))
port.write(frame(TXPOWER, bytes([TXP_DBM])))
port.write(frame(SF, bytes([SPREADING])))
port.write(frame(CR, bytes([CODING])))
port.write(frame(RADIO_STATE, bytes([1])))
port.flush()

print("listening %ds -- every device-to-host frame follows\n" % SECONDS)
deadline = time.time() + SECONDS
buf = bytearray()
in_frame = False
escaped = False
counts = {}
data_frames = 0

while time.time() < deadline:
    chunk = port.read(512)
    for byte in chunk:
        if byte == FEND:
            if in_frame and buf:
                command, payload = buf[0], bytes(buf[1:])
                name = NAMES.get(command, "0x%02x" % command)
                counts[name] = counts.get(name, 0) + 1
                if command == STAT_RSSI and payload:
                    print("  STAT_RSSI  %d dBm" % (payload[0] - RSSI_OFFSET))
                elif command == STAT_SNR and payload:
                    print("  STAT_SNR   raw=%d" % payload[0])
                elif command == RADIO_STATE:
                    print("  RADIO_STATE -> %s" % ("ON" if payload and payload[0] else "OFF (refused)"))
                elif command == ERROR:
                    print("  ERROR code=%s" % payload.hex())
                elif command == DATA:
                    data_frames += 1
                    print("  DATA  len=%d" % len(payload))
                    print("        hex=%s" % payload[:64].hex())
                else:
                    print("  %-11s len=%d %s" % (name, len(payload), payload[:16].hex()))
            buf.clear()
            in_frame = True
            escaped = False
            continue
        if not in_frame:
            continue
        if escaped:
            buf.append(FEND if byte == TFEND else FESC if byte == TFESC else byte)
            escaped = False
        elif byte == FESC:
            escaped = True
        else:
            buf.append(byte)

port.close()
print("\nframes by type: %s" % counts)
print("DATA frames: %d" % data_frames)
