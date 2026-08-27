//! First-flash discovery and re-enumeration evidence.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::device::{
    BoardSelection, BootloaderObservation, DeviceObservation, DeviceTransport, FirmwareState,
};
use crate::package::ProcessorKind;

pub const T114_LOADER_SNAPSHOT_SCHEMA: u32 = 1;

/// Facts captured from the T114's own UF2 information file before moving to serial DFU.
///
/// The volume is a distinct bootloader interface, so its record is kept intact instead of
/// pretending a silent application or its future serial port reported these facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct T114LoaderSnapshot {
    pub schema: u32,
    pub model: String,
    pub uf2_bootloader: String,
    pub softdevice: String,
    pub processor: ProcessorKind,
    pub flash_size: u32,
}

impl T114LoaderSnapshot {
    pub fn from_uf2_info(info: &str) -> Result<Self, DiscoveryError> {
        let uf2_bootloader = info
            .lines()
            .find_map(|line| line.trim().strip_prefix("UF2 Bootloader "))
            .and_then(|description| description.split_whitespace().next())
            .and_then(|version| version.split('-').next())
            .filter(|version| !version.is_empty())
            .ok_or_else(|| {
                DiscoveryError::T114Info("missing Adafruit UF2 bootloader version".into())
            })?;
        let model = info
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("Model: ")
                    .or_else(|| line.trim().strip_prefix("Board-ID: "))
            })
            .filter(|model| *model == "HT-n5262")
            .ok_or_else(|| {
                DiscoveryError::T114Info("the volume does not identify HT-n5262".into())
            })?;
        let softdevice = info
            .lines()
            .find_map(|line| line.trim().strip_prefix("SoftDevice: "))
            .filter(|softdevice| softdevice.to_ascii_lowercase().starts_with("s140 6."))
            .ok_or_else(|| DiscoveryError::T114Info("missing S140 v6 SoftDevice record".into()))?;
        Ok(Self {
            schema: T114_LOADER_SNAPSHOT_SCHEMA,
            model: model.into(),
            uf2_bootloader: uf2_bootloader.into(),
            softdevice: softdevice.into(),
            processor: ProcessorKind::Nrf52840,
            flash_size: 1024 * 1024,
        })
    }

    pub fn from_json(path: impl AsRef<Path>) -> Result<Self, DiscoveryError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|error| {
            DiscoveryError::Snapshot(format!("cannot read {}: {error}", path.display()))
        })?;
        let snapshot: Self = serde_json::from_str(&text).map_err(|error| {
            DiscoveryError::Snapshot(format!("cannot parse {}: {error}", path.display()))
        })?;
        if snapshot.schema != T114_LOADER_SNAPSHOT_SCHEMA {
            return Err(DiscoveryError::Snapshot(format!(
                "{} has schema {}, expected {}",
                path.display(),
                snapshot.schema,
                T114_LOADER_SNAPSHOT_SCHEMA
            )));
        }
        if snapshot.model != "HT-n5262"
            || snapshot.processor != ProcessorKind::Nrf52840
            || snapshot.flash_size != 1024 * 1024
            || !snapshot
                .softdevice
                .to_ascii_lowercase()
                .starts_with("s140 6.")
        {
            return Err(DiscoveryError::Snapshot(format!(
                "{} is not an HT-n5262 nRF52840 S140 v6 record",
                path.display()
            )));
        }
        Ok(snapshot)
    }

    pub fn save_json(&self, path: impl AsRef<Path>) -> Result<(), DiscoveryError> {
        let path = path.as_ref();
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| DiscoveryError::Snapshot(error.to_string()))?;
        std::fs::write(path, text).map_err(|error| {
            DiscoveryError::Snapshot(format!("cannot write {}: {error}", path.display()))
        })
    }

    pub fn uf2_bootloader_observation(&self, volume: &str) -> BootloaderObservation {
        BootloaderObservation {
            identifier: Some(volume.into()),
            descriptor: Some(format!("UF2 Bootloader v{}", self.uf2_bootloader)),
            processor: Some(self.processor.clone()),
            flash_size: Some(self.flash_size),
            bootloader: Some(format!("adafruit-uf2-{}", self.uf2_bootloader)),
            usb_vid: None,
            usb_pid: None,
        }
    }

    pub fn serial_dfu_observation(&self) -> BootloaderObservation {
        BootloaderObservation {
            identifier: Some(self.model.clone()),
            descriptor: Some(format!("UF2 record; SoftDevice {}", self.softdevice)),
            processor: Some(self.processor.clone()),
            flash_size: Some(self.flash_size),
            bootloader: Some("s140-v6".into()),
            usb_vid: None,
            usb_pid: None,
        }
    }
}

/// Read the T114's own bootloader record from its mounted UF2 volume.
pub fn t114_loader_snapshot_from_volume(
    volume: impl AsRef<Path>,
) -> Result<T114LoaderSnapshot, DiscoveryError> {
    let volume = volume.as_ref();
    if !volume.is_dir() {
        return Err(DiscoveryError::T114Info(format!(
            "{} is not an accessible mounted volume",
            volume.display()
        )));
    }
    let info_path = volume.join("INFO_UF2.TXT");
    let info = std::fs::read_to_string(&info_path).map_err(|error| {
        DiscoveryError::T114Info(format!("cannot read {}: {error}", info_path.display()))
    })?;
    T114LoaderSnapshot::from_uf2_info(&info)
}

/// Build a Linkboy observation for a mounted, self-identifying T114 UF2 loader.
///
/// The returned snapshot is deliberately separate so a later serial-DFU recovery can retain
/// the bootloader's physical record rather than borrowing facts from a foreign application.
pub fn t114_uf2_observation(
    volume: impl AsRef<Path>,
) -> Result<(DeviceObservation, T114LoaderSnapshot), DiscoveryError> {
    let volume = volume.as_ref();
    let snapshot = t114_loader_snapshot_from_volume(volume)?;
    let transport = volume.to_string_lossy().into_owned();
    let observation = DeviceObservation::from_bootloader(
        DeviceTransport::MountedVolume(transport.clone()),
        snapshot.uf2_bootloader_observation(&transport),
    );
    Ok((observation, snapshot))
}

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
    #[error("invalid T114 UF2 information: {0}")]
    T114Info(String),
    #[error("invalid T114 loader snapshot: {0}")]
    Snapshot(String),
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
    fn t114_loader_snapshot_preserves_the_uf2_and_serial_dfu_facts() {
        let snapshot = T114LoaderSnapshot::from_uf2_info(
            "UF2 Bootloader 0.9.0-2-g1234567\nModel: HT-n5262\nSoftDevice: S140 6.1.1\n",
        )
        .expect("the board's own INFO_UF2.TXT record is sufficient");
        assert_eq!(snapshot.model, "HT-n5262");
        assert_eq!(snapshot.uf2_bootloader, "0.9.0");
        assert_eq!(snapshot.softdevice, "S140 6.1.1");
        assert_eq!(snapshot.flash_size, 1024 * 1024);
        assert_eq!(
            snapshot.serial_dfu_observation().bootloader.as_deref(),
            Some("s140-v6")
        );
        assert_eq!(
            snapshot
                .uf2_bootloader_observation("D:\\")
                .bootloader
                .as_deref(),
            Some("adafruit-uf2-0.9.0")
        );
    }

    #[test]
    fn t114_loader_snapshot_refuses_an_incomplete_uf2_record() {
        let error = T114LoaderSnapshot::from_uf2_info("UF2 Bootloader 0.9.0\nModel: HT-n5262\n")
            .expect_err("SoftDevice evidence must not be invented");
        assert!(matches!(error, DiscoveryError::T114Info(_)));
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
            native_node_state: crate::device::NativeNodeState::Unknown,
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
