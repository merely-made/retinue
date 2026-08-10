# Signalman V4 graphical owner-route receipt

**Date:** 2026-08-10

**Board:** Heltec V4, COM6, revision 4.2

**Scope:** V4 only. `SIGNALMAN_SERIAL_PORTS=COM6` constrained the desktop
survey, so the T114 on COM3 was neither opened nor selected.

## Owner route

Signalman ran its real Cambium desktop flow against COM6. The route was
operated with keyboard input only:

1. Tab selected `COM6 — HeltecV4, region US915, channel rnode`; Space chose it.
2. Tab reached the board-revision field and entered `4.2`.
3. Tab/Enter used the device, selected `retinue.heltec-v4 — Partial`, reviewed
   the immutable plan, approved its stated changes, and started the install.

The live graphical result page said:

| Field | Result |
| --- | --- |
| Result | Complete |
| Running | HeltecV4 0.0.1 |
| Package | `retinue.heltec-v4` |
| Artifact | `application in its container: 7f5680ee0eb9a8d3a68eda62cd7f47b098ecb24f8096ce10d0f536a2d175fa7a` |
| Board | HeltecV4 4.2 |

An independent, post-window Linkboy status request to COM6 confirmed
`tulle/heltec-v4 phy online; version=0.0.1` and `identity=loaded slot=A seq=6`.
The structured terminal receipt is
[2026-08-10_linkboy_v4_com6_receipt.json](2026-08-10_linkboy_v4_com6_receipt.json).

## Boundary left open

This proves the V4 graphical route, keyboard navigation, execution, and
post-write application report. It does not prove manual screen-reader use or
recovery. During the completed page, Windows UI Automation exposed only the
root group even though the visible page rendered the complete receipt; that is
a runtime accessibility discrepancy to resolve before claiming the manual
screen-reader portion of G4. The T114 was intentionally untouched.
