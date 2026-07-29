# Direct-PHY host snapshot acceptance

**Date:** 2026-07-28
**Status:** complete; automated, headed command/RF, named-page, and expiry
receipts passed
**Plan rung:** U3 in `2026-07-28_on_device_ui_implementation_plan.md`

## Boundary

`radio-face` owns the versioned, allocation-free `HostSnapshot` schema and its
privacy/validity rules. Tulle carries an opaque payload and waits for a
firmware acknowledgement. Board firmware decodes the snapshot at the UI edge,
records receipt time, and removes it when `valid_for_secs` elapses.

The command is:

```text
03 <lowercase-hex radio-face payload> 00
```

The zero-free body and zero delimiter give a shortened outer command a real
recovery boundary. After an acknowledgement timeout the host writes one zero
before its next command. TX/config payload zeros remain ordinary data because
the parser treats zero specially only inside a UI-snapshot body or at a
command boundary.

The result event is:

```text
85 <result>
```

Results are zero accepted, one malformed, two unsupported version, and three
oversized.

## Parser and host receipts

- snapshot commands reassemble when split at every byte
- a shortened outer snapshot ends at the next wake byte and does not consume
  the following configure command
- an oversized snapshot is discarded through its delimiter and the following
  configure command is recovered
- a timed-out snapshot makes `DirectPhySerialLink` resynchronize before the
  following transmit
- the low-power UART wake preamble covers snapshot commands
- Tulle's public API accepts bytes and does not depend on `radio-face`
- the existing Sennet direct-PHY examples and Tucket `meshcore_headed` example
  compile unchanged

## Board command receipts

The production v14 T114 and the production V4 image each passed:

```text
UI SNAPSHOT future REJECTED result=2
UI SNAPSHOT RECOVERY ACCEPTED
UI SNAPSHOT truncated REJECTED result=1
UI SNAPSHOT RECOVERY ACCEPTED
UI SNAPSHOT minimal ACCEPTED
UI SNAPSHOT named ACCEPTED
```

The fixture event is `Info: HOST SNAPSHOT`. It is not a delivery or
propagation claim. Those real Retinue projections remain U4.

## RF non-regression

After the final v14 framing change, the existing bidirectional 4 KiB Retinue
Resource receipt passed:

```text
radios online: COM6=client, COM10=server
discovery: resource destination announced over direct PHY
publish: client to server 4096 bytes passed in 27.9s
fetch: server to client 4096 bytes passed in 25.7s
RETINUE DIRECT-PHY RESOURCE HEADED PASSED
```

COM labels are observations for this receipt, not persistent identities.

## Build receipts

- `radio-face`: 17 tests passed
- `selvage`: 9 tests passed
- Tulle: 36 unit tests and 5 capture tests passed
- strict no-dependency Clippy passed for the changed host crates and both
  firmware targets
- Rust 1.88 locked/offline check passed for Selvage and Tulle
- locked T114 and ESP32-S3 V4 release builds passed
- T114 v14 binary: 73,506 bytes; DFU ZIP: 74,382 bytes
- the final T114 release binary reproduced the flashed v14 binary byte for byte
- V4 application/partition usage: 165,968 / 16,384,000 bytes (1.01%)

## Physical receipt

The T114 v15 diagnostic image added `host=none|pending|fresh` to the existing
`ui\n` probe. It made no display or RF grammar change. After the named fixture
was injected, the probe reported:

```text
ui=ok; display=on; screen=traffic; button=p1.10; host=fresh; tft=write-only
```

The fitted button then traversed:

```text
STATUS -> POWER -> RADIO -> TRAFFIC -> IDENTITY -> LINKS -> PEERS
```

This proves the named snapshot changes the live page registry rather than
merely decoding at the USB edge. The injected fixture was `HERALD`; codec and
render tests cover its bounded identity, link, and peer content. The physical
receipt confirms the three resulting page titles, not a character-by-character
reading of every field.

The periodic named-snapshot refresher was stopped and the five-second expiry
fixture was injected. After expiry, physical navigation returned to:

```text
STATUS -> POWER -> RADIO -> TRAFFIC -> STATUS
```

The host-only pages were absent again. The snapshot therefore expires from the
board's receipt clock and does not leave stale identity, link, or peer truth on
the display.

The flashed v15 T114 binary is 73,666 bytes and its serial-DFU ZIP is 74,542
bytes.
