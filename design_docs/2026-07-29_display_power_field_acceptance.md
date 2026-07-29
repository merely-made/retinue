# Display power and field-behavior acceptance

**Date:** 2026-07-29
**Status:** USB-first V4 and T114 receipts passed; metered receipts remain open
**Plan rung:** U5 in `2026-07-28_on_device_ui_implementation_plan.md`

## Boundary

This pass proves display-power behavior with the connected USB radios. It does
not claim CPU Light-sleep, current, or energy. Those remain separate and
meter-gated.

## V4 display and wake

The fitted V4 button selected DISPLAY OFF through the one-button menu. The OLED
rendered `KEY TO WAKE`, powered down, and consumed the next press while
restoring STATUS. A USB diagnostic after the wake reported:

```text
ui=ok; display=on; screen=status; button=1
```

All 17 `radio-face` controller/render tests passed, including consumed wake and
healthy-idle LED-dark policy.

## Serial control-line finding

The first RF-continuity attempt was invalid: opening COM6 with the existing
DTR-asserted policy reset the V4 and restored STATUS before RF. Closing the
handle could repeat the same visible reset.

`DirectPhySerialConfig` now makes DTR and RTS explicit. The defaults preserve
the nRF USB CDC requirement (`dtr=true`) and the established ESP32 protection
(`rts=false`). The V4 native-USB side of the acceptance harness used
`dtr=false`, so neither attachment nor teardown introduced a DTR transition.

The Resource example also accepts bounded preflight and postflight holds
through environment settings. These keep both serial handles open around a
physical observation rather than confusing port attachment with radio
behavior.

## Display-off RF receipt

With the V4 already reporting DISPLAY OFF, the DTR-low harness opened COM6 and
the ordinary DTR-asserted CDC path opened the T114 on COM10. A 256-byte Resource
then passed byte-exact in both directions:

```text
radios online: COM6=client, COM10=server
interface: open with logical MTU 255
discovery: resource destination announced over direct PHY
publish: client to server 256 bytes passed in 4.3s
fetch: server to client 256 bytes passed in 6.6s
postflight: radios held open for 45s after RF
RETINUE DIRECT-PHY RESOURCE HEADED PASSED
```

After RF, the 45-second hold, process teardown, and a fresh DTR-low diagnostic
attachment, the firmware still reported:

```text
ui=ok; display=off; screen=display-off; button=1
```

This proves RF receive/transmit state updates do not permanently wake the V4
OLED and that the native-USB host path can attach and detach without resetting
the local display state.

## T114 display and wake

The fitted P1.10 button selected DISPLAY OFF from TRAFFIC. The TFT rendered the
off face, then its backlight powered down. One fitted-button press restored
TRAFFIC rather than advancing to STATUS. The USB diagnostics captured both
sides:

```text
ui=ok; display=off; screen=display-off; button=p1.10; host=none; tft=write-only
ui=ok; display=on; screen=traffic; button=p1.10; host=none; tft=write-only
```

This is the physical consumed-wake receipt for the write-only fitted panel.

## Both displays off during RF

The T114 was turned off again while the V4 remained off. The DTR-low V4 client
and ordinary DTR-asserted T114 CDC server then passed another 256-byte Resource
in both directions:

```text
radios online: COM6=client, COM10=server
interface: open with logical MTU 255
discovery: resource destination announced over direct PHY
publish: client to server 256 bytes passed in 2.0s
fetch: server to client 256 bytes passed in 4.6s
postflight: radios held open for 15s after RF
RETINUE DIRECT-PHY RESOURCE HEADED PASSED
```

Fresh diagnostics after RF, postflight, and teardown reported both displays
still off:

```text
COM6: ui=ok; display=off; screen=display-off; button=1
COM10: ui=ok; display=off; screen=display-off; button=p1.10; host=none; tft=write-only
```

RF status updates therefore leave both local display-power states intact.

## Verification

- `cargo test -p radio-face --locked`: 17 passed
- `cargo test -p tulle --all-features --locked`: 36 unit and 5 capture tests passed
- strict no-deps Clippy passed for Tulle and the Retinue hardware example
- formatting and `git diff --check` passed

## Open receipts

- OLED, TFT backlight, LED, and UART Light-sleep current measured separately
- display refresh shown not to keep the V4 low-power sleep gate blocked
- quiet and representative-workload energy against the display-less baseline
