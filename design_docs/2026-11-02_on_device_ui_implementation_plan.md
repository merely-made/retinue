# On-device radio UI: remaining work

**Status (2026-11-02): plan.** Successor to the [original UI plan](archive_docs/2026-11-02/2026-07-28_on_device_ui_implementation_plan.md), whose U0–U5 implementation and bounded receipts remain historical evidence. Design authority remains [on-device UI](2026-07-25_on_device_ui.md); physical power work also has the [display power acceptance](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-29_display_power_field_acceptance.md).

## Phases and done-conditions

1. **Meter display power and energy.** Measure OLED-off, TFT backlight-off, LED-off and UART Light-sleep separately on the exact V4 and T114 profiles. Done when refresh does not prevent the V4 sleep gate, radio wake updates state without leaving the display on, and quiet and representative workload energy is compared with the display-less firmware baseline. Identify instrument, artifacts and conditions. USB-first screen-off and RF continuity do not substitute for metering.
2. **Admit product states when their owners exist.** Retain editable PHY presets until host/device authority and refusal are explicit; BLE pairing until a real management channel performs numeric comparison; OTA until target image verification, rollback and progress facts exist; native provisioning, identity, peer and route ownership on the modem; and message text on the radio display. Done for each only when the owning feature's real facts and refusal states are available and the firmware menu accurately exposes them. Rendering a golden alone does not admit a feature.

## Decisions and constraints

The board is authoritative for local facts, the host for its existing projections. A signature or host snapshot is not proof of local peer, route or RF state. Preserve button wake, display-off and radio continuity as distinct facts. BLE and firmware activation follow their carrier and update owners; do not introduce a second authority through the face. Keep minimal display detail available and let host policy govern names and fingerprints. Coalesce display updates outside radio-critical work, and test fragmentation and resynchronization for any new wire command.

## Findings (2026-11-02)

`crates/radio-face/src/status.rs:216` defines local status, while `crates/radio-face/src/controller.rs:139` owns face controls. This source inspection does not add new display, radio or meter acceptance.

## Deferred and follow-on

The relocated V4 Light-sleep report also retains unqualified repeated RF-triggered sleep continuity, post-wake OLED restoration and cold physical UART wake. Qualify those exact target behaviors before combining the low-power and display paths; reconcile later owning evidence before treating an old observation as a current defect. The existing display-current and energy gate above remains separate.

The five product states above remain conditional on their respective owners; refer to the [wall-node plan](2026-11-02_wall_node_management_plan.md) and [Linkboy plan](2026-11-02_linkboy_public_flashing_plan.md) for management and image authority. Reconcile paths when those plans receive successors.

## Progress

- 2026-11-02: adopted the reviewed open-work successor; original archived whole. No new implementation or acceptance run.