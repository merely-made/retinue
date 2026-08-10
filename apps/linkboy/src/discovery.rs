//! First-flash discovery and re-enumeration evidence.

use std::collections::BTreeSet;

use thiserror::Error;

use crate::device::{BoardSelection, DeviceObservation, DeviceTransport, FirmwareState};

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DiscoveryError {
    #[error("no new bootloader port appeared")]
    NoNewPort,
    #[error("more than one new port appeared: {0:?}")]
    MultipleNewPorts(Vec<String>),
    #[error("selected board contradicts loader facts: {0}")]
    Contradiction(String),
    #[error("loader output did not contain enough hardware facts: {0}")]
    IncompleteFacts(String),
}

/// Pick a single newly enumerated port. A COM number is useful as a location observation only;
/// it is never copied into board identity.
pub fn unique_new_port(
    before: impl IntoIterator<Item = String>,
    after: impl IntoIterator<Item = String>,
) -> Result<String, DiscoveryError> {
    let before: BTreeSet<_> = before.into_iter().collect();
    let mut new_ports: Vec<_> = after
        .into_iter()
        .filter(|port| !before.contains(port))
        .collect();
    new_ports.sort();
    match new_ports.as_slice() {
        [port] => Ok(port.clone()),
        [] => Err(DiscoveryError::NoNewPort),
        ports => Err(DiscoveryError::MultipleNewPorts(ports.to_vec())),
    }
}

/// Construct a stock-device observation from route-specific loader facts. This is the F3 seam:
/// the application status reply is optional, while processor, flash size, and bootloader facts
/// come from the loader that actually saw the board.
pub fn stock_device(
    transport: DeviceTransport,
    bootloader: crate::device::BootloaderObservation,
    selection: BoardSelection,
) -> Result<DeviceObservation, DiscoveryError> {
    let observation = DeviceObservation::from_bootloader(transport, bootloader)
        .confirm_board(selection.family, selection.revision);
    if let Some(contradiction) = observation.contradictions.first() {
        return Err(DiscoveryError::Contradiction(contradiction.clone()));
    }
    if observation.hardware.processor.is_none()
        || observation.hardware.flash_size.is_none()
        || observation.hardware.bootloader.is_none()
    {
        return Err(DiscoveryError::IncompleteFacts(
            "processor, flash size, and bootloader are required".into(),
        ));
    }
    Ok(observation)
}

pub fn is_first_flash(observation: &DeviceObservation) -> bool {
    matches!(
        observation.firmware,
        FirmwareState::Bootloader | FirmwareState::Unknown
    ) && observation.status_reply.is_none()
}

/// Whether a V4 observation has enough of a claim to justify the ROM loader's
/// non-writing board-info probe. A running Retinue V4 has already named its
/// family; a silent board needs the owner's explicit V4 selection. A serial
/// location by itself is never enough to reset a board into its loader.
pub fn needs_esp_rom_probe(observation: &DeviceObservation) -> bool {
    matches!(
        observation
            .selected_board
            .as_ref()
            .map(|board| &board.family),
        Some(crate::package::BoardFamily::HeltecV4)
    ) || matches!(
        observation.firmware,
        FirmwareState::Retinue {
            family: crate::package::BoardFamily::HeltecV4
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{
        BootloaderObservation, DeviceTransport, EvidenceConfidence, HardwareFacts,
    };
    use crate::package::{BoardFamily, ProcessorKind};

    #[test]
    fn stock_v4_reaches_a_first_flash_observation() {
        let observation = stock_device(
            DeviceTransport::SerialPort("COM7".into()),
            BootloaderObservation {
                identifier: Some("ESP-ROM:esp32s3".into()),
                descriptor: Some("ESP32-S3 ROM".into()),
                processor: Some(ProcessorKind::Esp32S3),
                flash_size: Some(16 * 1024 * 1024),
                bootloader: Some("esp-rom".into()),
                usb_vid: Some(0x303a),
                usb_pid: Some(0x1001),
            },
            BoardSelection::owner_confirmed(BoardFamily::HeltecV4, "4.2"),
        )
        .unwrap();
        assert!(is_first_flash(&observation));
        assert_eq!(observation.confidence, EvidenceConfidence::OwnerConfirmed);
    }

    #[test]
    fn stock_t114_reaches_a_first_flash_observation() {
        let observation = stock_device(
            DeviceTransport::SerialPort("COM8".into()),
            BootloaderObservation {
                identifier: Some("nRF52840 DFU".into()),
                descriptor: Some("S140 v6".into()),
                processor: Some(ProcessorKind::Nrf52840),
                flash_size: Some(1024 * 1024),
                bootloader: Some("s140-v6".into()),
                usb_vid: Some(0x1915),
                usb_pid: Some(0x521f),
            },
            BoardSelection::owner_confirmed(BoardFamily::T114, "2.x"),
        )
        .unwrap();
        assert!(is_first_flash(&observation));
    }

    #[test]
    fn wrong_carrier_choice_is_refused_by_loader_evidence() {
        let error = stock_device(
            DeviceTransport::SerialPort("COM7".into()),
            BootloaderObservation {
                identifier: Some("ESP-ROM:esp32s3".into()),
                descriptor: None,
                processor: Some(ProcessorKind::Esp32S3),
                flash_size: Some(4 * 1024 * 1024),
                bootloader: Some("esp-rom".into()),
                usb_vid: None,
                usb_pid: None,
            },
            BoardSelection::owner_confirmed(BoardFamily::T114, "2.x"),
        )
        .expect_err("a T114 choice cannot override ESP loader facts");
        assert!(matches!(error, DiscoveryError::Contradiction(_)));
    }

    #[test]
    fn port_identity_does_not_survive_an_ambiguous_reenumeration() {
        let error = unique_new_port(["COM7".into()], ["COM8".into(), "COM9".into()])
            .expect_err("two new ports must not be guessed between");
        assert!(matches!(error, DiscoveryError::MultipleNewPorts(_)));
        assert_eq!(
            unique_new_port(["COM7".into()], ["COM7".into(), "COM8".into()]).unwrap(),
            "COM8"
        );
    }

    #[test]
    fn fixtures_are_not_status_dependent() {
        let v4 = crate::route::esp_rom::bootloader_facts(include_str!(
            "../tests/fixtures/esp-rom-v4.txt"
        ))
        .unwrap();
        let t114 = crate::route::adafruit_dfu::bootloader_facts(include_str!(
            "../tests/fixtures/t114-dfu.txt"
        ))
        .unwrap();
        assert_eq!(v4.flash_size, Some(16 * 1024 * 1024));
        assert_eq!(t114.flash_size, Some(1024 * 1024));
        let observation = DeviceObservation {
            transport: DeviceTransport::SerialPort("COM10".into()),
            status_reply: None,
            hardware: HardwareFacts {
                processor: Some(ProcessorKind::Nrf52840),
                flash_size: Some(1024 * 1024),
                bootloader: Some("s140-v6".into()),
                loader_route: Some("serial-dfu".into()),
                bootloader_usb: None,
            },
            selected_board: Some(BoardSelection::owner_confirmed(BoardFamily::T114, "2.x")),
            firmware: FirmwareState::Bootloader,
            confidence: EvidenceConfidence::OwnerConfirmed,
            contradictions: Vec::new(),
        };
        assert!(is_first_flash(&observation));
    }

    #[test]
    fn a_silent_owner_selected_v4_is_worth_a_loader_probe() {
        let v4 = DeviceObservation::from_found(&crate::Found {
            port: "COM7".into(),
            board: None,
            banner: String::new(),
            region: None,
            channel: None,
        })
        .confirm_board(BoardFamily::HeltecV4, "4.2");
        assert!(needs_esp_rom_probe(&v4));

        let t114 = DeviceObservation::from_found(&crate::Found {
            port: "COM8".into(),
            board: None,
            banner: String::new(),
            region: None,
            channel: None,
        })
        .confirm_board(BoardFamily::T114, "2.x");
        assert!(
            !needs_esp_rom_probe(&t114),
            "a T114 selection cannot borrow V4 loader evidence"
        );
    }
}
