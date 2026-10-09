//! Signalman's projection of Linkboy's owner installer.
//!
//! Semantic rather than terminal-shaped: a window renders the view with its own controls while
//! Linkboy remains the only owner of package policy, plans, and execution events.

// Firmware errors inherit Linkboy's deliberately large recovery payloads; flashing is a
// cold path where truncated evidence costs more than a wide Err.
#![allow(clippy::result_large_err)]

mod device;
mod installer;
#[cfg(test)]
mod tests;

pub use device::{
    DeviceCandidate, capture_t114_uf2_volume, describe_event, event_progress, observe_device,
    observe_device_with_board_selection_and_t114_loader_snapshot,
    observe_device_with_t114_loader_snapshot, observe_t114_serial_dfu_port,
    observe_t114_uf2_volume, refusal_lines, survey_devices, survey_ports,
};
pub use installer::{
    FirmwareCatalog, FirmwareError, FirmwareInstallNotice, FirmwareInstallRecovery,
    FirmwareInstallStage, FirmwareInstallUpdate, FirmwareInstallWorker, FirmwareInstaller,
    FirmwareReview, FirmwareView, InstallerWake,
};
