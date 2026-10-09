mod rediscovery;
mod route;
mod uf2;

use super::device::{matches_expected_family, select_application_port};
use super::uf2_volume::uf2_volume_ejected_after_write;
use super::*;
use crate::device::{
    BoardSelection, DeviceObservation, DeviceTransport, EvidenceConfidence, FirmwareState,
    HardwareFacts,
};
use crate::package::{
    BoardFamily, ExpectedApplication, FirmwarePartKind, FlashPackage, FlashPackageManifest,
    FlashRange, FlashRoute, PACKAGE_SCHEMA, PackagePart, PackagePayload, PackageTarget,
    PayloadFormat, RecoveryInstructions, StateImpact,
};
use crate::plan::{CompatibilityFact, FlashPlan, PackageIdentity, PlanWarning};
use crate::receipt::ApplicationVerification;
use crate::route::{adafruit_dfu, esp_rom};

struct MockProcess {
    result: Result<ProcessOutput, ProcessFailure>,
    progress: Vec<ProcessProgress>,
}

impl ProcessRunner for MockProcess {
    fn run(
        &mut self,
        _program: &str,
        _args: &[String],
        progress: &mut dyn FnMut(ProcessProgress),
    ) -> Result<ProcessOutput, ProcessFailure> {
        for value in self.progress.clone() {
            progress(value);
        }
        self.result.clone()
    }
}

#[derive(Default)]
struct RecordingProcess {
    calls: Vec<Vec<String>>,
}

impl ProcessRunner for RecordingProcess {
    fn run(
        &mut self,
        _program: &str,
        args: &[String],
        progress: &mut dyn FnMut(ProcessProgress),
    ) -> Result<ProcessOutput, ProcessFailure> {
        self.calls.push(args.to_vec());
        progress(ProcessProgress {
            written: 100,
            total: 100,
        });
        Ok(ProcessOutput {
            diagnostics: "write 100%".into(),
        })
    }
}

struct MockDevice {
    bootloader: Result<String, DeviceFailure>,
    application: Result<String, DeviceFailure>,
    verification: Result<ApplicationVerification, DeviceFailure>,
}

impl DeviceRunner for MockDevice {
    fn enter_bootloader(
        &mut self,
        _current_port: &str,
        _patience: Duration,
    ) -> Result<String, DeviceFailure> {
        self.bootloader.clone()
    }

    fn rediscover_application(
        &mut self,
        _original_port: &str,
        _bootloader_port: &str,
        _expected: &ExpectedApplication,
        _patience: Duration,
    ) -> Result<String, DeviceFailure> {
        self.application.clone()
    }

    fn verify_application(
        &mut self,
        _application_port: &str,
        _expected: &ExpectedApplication,
    ) -> Result<ApplicationVerification, DeviceFailure> {
        self.verification.clone()
    }
}

fn package(route: FlashRoute) -> FlashPackage {
    let manual_check = matches!(route, FlashRoute::Uf2MassStorage)
        .then(|| "Exercise the upstream interface.".into());
    package_with_manual_check(route, manual_check)
}

fn package_with_manual_check(route: FlashRoute, manual_check: Option<String>) -> FlashPackage {
    let bytes = match route {
        FlashRoute::Uf2MassStorage => test_uf2_bytes(),
        FlashRoute::AdafruitDfu | FlashRoute::EspRom => b"payload".to_vec(),
    };
    let (family, processor, bootloader, format, revision) = match route {
        FlashRoute::AdafruitDfu => (
            BoardFamily::T114,
            crate::package::ProcessorKind::Nrf52840,
            "s140-v6",
            PayloadFormat::NrfDfuZip,
            "2.x",
        ),
        FlashRoute::EspRom => (
            BoardFamily::HeltecV4,
            crate::package::ProcessorKind::Esp32S3,
            "esp-rom",
            PayloadFormat::EspflashElf,
            "4.2",
        ),
        FlashRoute::Uf2MassStorage => (
            BoardFamily::T114,
            crate::package::ProcessorKind::Nrf52840,
            "adafruit-uf2-0.9.0",
            PayloadFormat::Uf2,
            "2.x",
        ),
    };
    let manifest = FlashPackageManifest {
        schema: PACKAGE_SCHEMA,
        package_id: "test".into(),
        display_name: "Test".into(),
        version: "1".into(),
        publisher: "Test".into(),
        helpers: vec![crate::package::HelperRequirement {
            route: route.clone(),
            program: route.helper().into(),
            version: match route {
                FlashRoute::AdafruitDfu => "0.5.3.post16",
                FlashRoute::EspRom => "4.5.0",
                FlashRoute::Uf2MassStorage => "0.0.1",
            }
            .into(),
            binary_sha256: None,
            artifacts: Vec::new(),
            license: "test".into(),
            source_url: "https://example.invalid/helper".into(),
            notice: "Test helper notice".into(),
        }],
        payload: Some(PackagePayload {
            path: match route {
                FlashRoute::Uf2MassStorage => "payload.uf2",
                FlashRoute::AdafruitDfu | FlashRoute::EspRom => "payload",
            }
            .into(),
            format,
            byte_length: bytes.len() as u64,
            sha256: crate::package::sha256_hex(&bytes),
            write_bytes: match route {
                FlashRoute::Uf2MassStorage => 4,
                FlashRoute::AdafruitDfu | FlashRoute::EspRom => bytes.len() as u64,
            },
        }),
        parts: Vec::new(),
        targets: vec![PackageTarget {
            family: family.clone(),
            revision: revision.into(),
            processor,
            flash_size: match route {
                FlashRoute::Uf2MassStorage => 1024 * 1024,
                FlashRoute::AdafruitDfu | FlashRoute::EspRom => 4 * 1024 * 1024,
            },
            bootloader: bootloader.into(),
            route: route.clone(),
        }],
        write_ranges: match route {
            FlashRoute::Uf2MassStorage => vec![FlashRange {
                start: 0x26000,
                length: 4,
            }],
            FlashRoute::AdafruitDfu | FlashRoute::EspRom => vec![FlashRange {
                start: 0,
                length: 1,
            }],
        },
        preserved_ranges: match route {
            FlashRoute::Uf2MassStorage => vec![FlashRange {
                start: 0x26004,
                length: 1,
            }],
            FlashRoute::AdafruitDfu | FlashRoute::EspRom => vec![FlashRange {
                start: 1,
                length: 1,
            }],
        },
        regions: vec!["US915".into()],
        channel_capabilities: vec!["modem".into(), "node".into(), "rnode".into()],
        state_impact: StateImpact::Preserved,
        expected_application: ExpectedApplication {
            board: family,
            version: "0.0.1".into(),
            manual_check,
        },
        license: "MPL-2.0".into(),
        notices: "Notices".into(),
        source_revision: "test".into(),
        source_url: "https://example.invalid/source".into(),
        origin_url: "https://example.invalid/package".into(),
        publisher_signature: None,
        recovery: RecoveryInstructions {
            before_write: "Keep cable attached.".into(),
            after_failure: "Use bootloader recovery.".into(),
        },
        persistent_state: None,
    };
    let payload_path = match route {
        FlashRoute::Uf2MassStorage => "payload.uf2",
        FlashRoute::AdafruitDfu | FlashRoute::EspRom => "payload",
    };
    FlashPackage::from_parts(manifest, "manifest", payload_path, bytes).unwrap()
}

fn test_uf2_bytes() -> Vec<u8> {
    let mut block = vec![0_u8; 512];
    let word = |block: &mut [u8], offset: usize, value: u32| {
        block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    word(&mut block, 0, 0x0A32_4655_u32);
    word(&mut block, 4, 0x9E5D_5157_u32);
    word(&mut block, 8, 0x0000_2000_u32);
    word(&mut block, 12, 0x26000_u32);
    word(&mut block, 16, 4_u32);
    word(&mut block, 20, 0_u32);
    word(&mut block, 24, 1_u32);
    word(&mut block, 28, crate::uf2::NRF52840_FAMILY_ID);
    block[32..36].copy_from_slice(b"UF2!");
    word(&mut block, 508, 0x0AB1_6F30_u32);
    block
}

fn sparse_esp_package() -> FlashPackage {
    let bootloader = b"bootloader".to_vec();
    let partition_table = b"partition-table".to_vec();
    let application = b"application".to_vec();
    let parts = [
        (
            "bootloader.bin",
            FirmwarePartKind::Bootloader,
            0,
            bootloader,
        ),
        (
            "partition-table.bin",
            FirmwarePartKind::PartitionTable,
            0x8000,
            partition_table,
        ),
        (
            "application.bin",
            FirmwarePartKind::Application,
            0x10000,
            application,
        ),
    ];
    let manifest = FlashPackageManifest {
        schema: PACKAGE_SCHEMA,
        package_id: "upstream.hopspot-v4".into(),
        display_name: "Upstream Hopspot for Heltec V4".into(),
        version: "test".into(),
        publisher: "Upstream".into(),
        helpers: vec![crate::package::HelperRequirement {
            route: FlashRoute::EspRom,
            program: "espflash".into(),
            version: "4.5.0".into(),
            binary_sha256: None,
            artifacts: Vec::new(),
            license: "MIT OR Apache-2.0".into(),
            source_url: "https://example.invalid/espflash".into(),
            notice: "Test helper notice".into(),
        }],
        payload: None,
        parts: parts
            .iter()
            .map(|(path, kind, offset, bytes)| PackagePart {
                kind: kind.clone(),
                path: (*path).into(),
                format: PayloadFormat::RawBinary,
                offset: Some(*offset),
                byte_length: bytes.len() as u64,
                sha256: crate::package::sha256_hex(bytes),
                write_bytes: bytes.len() as u64,
            })
            .collect(),
        targets: vec![PackageTarget {
            family: BoardFamily::HeltecV4,
            revision: "4.2".into(),
            processor: crate::package::ProcessorKind::Esp32S3,
            flash_size: 4 * 1024 * 1024,
            bootloader: "esp-rom".into(),
            route: FlashRoute::EspRom,
        }],
        write_ranges: Vec::new(),
        preserved_ranges: vec![FlashRange {
            start: 0xd000,
            length: 0x1000,
        }],
        regions: vec!["US915".into()],
        channel_capabilities: vec!["modem".into()],
        state_impact: StateImpact::Unknown,
        expected_application: ExpectedApplication {
            board: BoardFamily::HeltecV4,
            version: "test".into(),
            manual_check: Some("Exercise the upstream interface.".into()),
        },
        license: "MPL-2.0".into(),
        notices: "Test notices".into(),
        source_revision: "test".into(),
        source_url: "https://example.invalid/source".into(),
        origin_url: "https://example.invalid/package".into(),
        publisher_signature: None,
        recovery: RecoveryInstructions {
            before_write: "Keep the cable attached.".into(),
            after_failure: "Enter the ROM loader again.".into(),
        },
        persistent_state: None,
    };
    FlashPackage::from_verified_parts(
        manifest,
        "manifest",
        parts
            .into_iter()
            .map(|(path, _, _, bytes)| (path.into(), bytes))
            .collect(),
    )
    .unwrap()
}

fn plan(route: FlashRoute) -> FlashPlan {
    plan_with_transport(route, DeviceTransport::SerialPort("COM7".into()))
}

fn plan_with_transport(route: FlashRoute, transport: DeviceTransport) -> FlashPlan {
    let (family, processor, bootloader, revision) = match route {
        FlashRoute::AdafruitDfu => (
            BoardFamily::T114,
            crate::package::ProcessorKind::Nrf52840,
            "s140-v6",
            "2.x",
        ),
        FlashRoute::EspRom => (
            BoardFamily::HeltecV4,
            crate::package::ProcessorKind::Esp32S3,
            "esp-rom",
            "4.2",
        ),
        FlashRoute::Uf2MassStorage => (
            BoardFamily::T114,
            crate::package::ProcessorKind::Nrf52840,
            "adafruit-uf2-0.9.0",
            "2.x",
        ),
    };
    FlashPlan::for_test(
        DeviceObservation {
            transport,
            status_reply: None,
            hardware: HardwareFacts {
                processor: Some(processor),
                flash_size: Some(4 * 1024 * 1024),
                bootloader: Some(bootloader.into()),
                loader_route: None,
                bootloader_usb: None,
            },
            selected_board: Some(BoardSelection::owner_confirmed(family.clone(), revision)),
            firmware: FirmwareState::Retinue {
                family: family.clone(),
            },
            confidence: EvidenceConfidence::OwnerConfirmed,
            contradictions: Vec::new(),
            native_node_state: crate::device::NativeNodeState::Unknown,
        },
        PackageIdentity {
            package_id: "test".into(),
            display_name: "Test".into(),
            version: "1".into(),
            parts: vec![crate::PackagePartIdentity {
                kind: crate::FirmwarePartKind::Application,
                offset: None,
                byte_length: 1,
                sha256: "a".repeat(64),
            }],
            publisher_signature: None,
        },
        BoardSelection::owner_confirmed(family, revision),
        route,
        vec![],
        vec![],
        StateImpact::Preserved,
        vec![CompatibilityFact {
            name: "board".into(),
            value: "confirmed".into(),
            source: "test".into(),
        }],
        vec![PlanWarning {
            message: "warning".into(),
            requires_confirmation: false,
        }],
        "before".into(),
        "after".into(),
    )
}

fn uf2_volume_plan(volume: String) -> FlashPlan {
    FlashPlan::for_test(
        DeviceObservation {
            transport: DeviceTransport::MountedVolume(volume),
            status_reply: None,
            hardware: HardwareFacts {
                processor: Some(crate::package::ProcessorKind::Nrf52840),
                flash_size: Some(1024 * 1024),
                bootloader: Some("adafruit-uf2-0.9.0".into()),
                loader_route: Some("uf2-mass-storage".into()),
                bootloader_usb: None,
            },
            selected_board: Some(BoardSelection::owner_confirmed(BoardFamily::T114, "2.x")),
            firmware: FirmwareState::Bootloader,
            confidence: EvidenceConfidence::OwnerConfirmed,
            contradictions: Vec::new(),
            native_node_state: crate::device::NativeNodeState::Unknown,
        },
        PackageIdentity {
            package_id: "test".into(),
            display_name: "Test".into(),
            version: "1".into(),
            parts: vec![crate::PackagePartIdentity {
                kind: crate::FirmwarePartKind::Application,
                offset: None,
                byte_length: 512,
                sha256: crate::package::sha256_hex(&test_uf2_bytes()),
            }],
            publisher_signature: None,
        },
        BoardSelection::owner_confirmed(BoardFamily::T114, "2.x"),
        FlashRoute::Uf2MassStorage,
        vec![FlashRange {
            start: 0x26000,
            length: 4,
        }],
        vec![FlashRange {
            start: 0x26004,
            length: 1,
        }],
        StateImpact::Unknown,
        vec![CompatibilityFact {
            name: "bootloader".into(),
            value: "UF2".into(),
            source: "test".into(),
        }],
        Vec::new(),
        "before".into(),
        "after".into(),
    )
}

fn success_device() -> MockDevice {
    MockDevice {
        bootloader: Ok("DFU1".into()),
        application: Ok("COM7".into()),
        verification: Ok(ApplicationVerification {
            board: BoardFamily::HeltecV4,
            version: "0.0.1".into(),
            region: Some("US915".into()),
            channel: Some("rnode".into()),
        }),
    }
}
