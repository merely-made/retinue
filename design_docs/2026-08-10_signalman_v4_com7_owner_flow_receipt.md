# Signalman V4 COM7 owner-flow receipt

Date: 2026-08-10

Scope: `COM7` only. Signalman was launched with
`SIGNALMAN_SERIAL_PORTS=COM7`; no action opened, reset, flashed, or surveyed
`COM3` / T114.

This run began after Linkboy had already restored Retinue on COM7. It proves a
graphical Retinue-to-Retinue owner flow, not the preceding Hopspot-to-Retinue
restore. The separate Hopspot interface receipt keeps that distinction explicit.

## Graphical owner flow

The private `signalman-desktop` application performed these visible controls
through Windows UI Automation:

1. selected `COM7 — HeltecV4, region US915, channel rnode`;
2. explicitly selected `Use V4 revision 4.2` (not a prefilled value);
3. used `Use this device`, which advanced after Signalman observed COM7;
4. selected `retinue.heltec-v4` and did not act on the visible T114 package;
5. reviewed the resulting plan, approved it, and started the reviewed install
   from the preparation page.

The COM7 page and firmware page changes prove those GUI requests reached
Signalman's owner flow. The GUI was built after this flow's V4 revision choice
was added and its focused owner-flow test passed.

## Post-run device evidence

After the GUI-launched install settled, the COM7-only status command reported:

```text
tulle/heltec-v4 phy online; version=0.0.1; sx1262 online; sync=2b reg=24b4; longfast=906875000
identity=loaded slot=A seq=4
```

This is a live Retinue V4 banner and loaded identity on COM7.

## Remaining GUI and restore evidence gaps

The final static Signalman result panel did not expose named descendant nodes
to Windows UI Automation, so it could not independently state the GUI worker's
terminal receipt. The live COM7 banner is separate device evidence. General
accessible value-setting for Cambium text fields remains a Genet host follow-up;
the explicit V4 revision choice is an owner-selected, board-specific path used
for this V4 acceptance run.

Because this run started from Retinue, graphical restoration from an external
firmware remains open.
