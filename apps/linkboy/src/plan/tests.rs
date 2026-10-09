use super::*;
use crate::device::{DeviceTransport, FirmwareState, HardwareFacts, NativeNodeState};
use crate::package::{
    ExpectedApplication, FirmwarePartKind, FlashPackage, FlashPackageManifest,
    NODE_TIMEBASE_PRESERVED_RANGE, PACKAGE_SCHEMA, PERSISTENT_STATE_SCHEMA, PackagePart,
    PackagePayload, PackageTarget, PayloadFormat, PersistentStateCompatibility,
    RecoveryInstructions,
};

fn package() -> FlashPackage {
    let bytes = b"package-bytes".to_vec();
    let manifest = FlashPackageManifest {
        schema: PACKAGE_SCHEMA,
        package_id: "test.v4".into(),
        display_name: "Test V4".into(),
        version: "1".into(),
        publisher: "Test".into(),
        helpers: vec![crate::package::HelperRequirement {
            route: FlashRoute::EspRom,
            program: "espflash".into(),
            version: "4.5.0".into(),
            binary_sha256: None,
            artifacts: vec![crate::package::HelperArtifact {
                platform: crate::package::helper_platform(),
                binary_sha256: "a".repeat(64),
                archive_sha256: "b".repeat(64),
                archive_url: "https://example.invalid/espflash.tar.gz".into(),
            }],
            license: "MIT OR Apache-2.0".into(),
            source_url: "https://example.invalid/espflash".into(),
            notice: "Test helper notice".into(),
        }],
        payload: Some(PackagePayload {
            path: "payload".into(),
            format: PayloadFormat::EspflashElf,
            byte_length: bytes.len() as u64,
            sha256: crate::package::sha256_hex(&bytes),
            write_bytes: bytes.len() as u64,
        }),
        parts: Vec::new(),
        targets: vec![PackageTarget {
            family: BoardFamily::HeltecV4,
            revision: "4.2".into(),
            processor: ProcessorKind::Esp32S3,
            flash_size: 4 * 1024 * 1024,
            bootloader: "esp-rom".into(),
            route: FlashRoute::EspRom,
        }],
        write_ranges: vec![FlashRange {
            start: 0,
            length: 0x3f0000,
        }],
        preserved_ranges: vec![FlashRange {
            start: 0x3f0000,
            length: 0x10000,
        }],
        regions: vec!["US915".into()],
        channel_capabilities: vec!["modem".into(), "rnode".into()],
        state_impact: StateImpact::Preserved,
        expected_application: ExpectedApplication {
            board: BoardFamily::HeltecV4,
            version: "0.0.1".into(),
            manual_check: None,
        },
        license: "MPL-2.0".into(),
        notices: "Notices".into(),
        source_revision: "test".into(),
        source_url: "https://example.invalid/source".into(),
        origin_url: "https://example.invalid/package".into(),
        publisher_signature: None,
        recovery: RecoveryInstructions {
            before_write: "Keep cable attached.".into(),
            after_failure: "Use ROM entry.".into(),
        },
        persistent_state: None,
    };
    FlashPackage::from_parts(manifest, "manifest", "payload", bytes).unwrap()
}

fn t114_package() -> FlashPackage {
    let bytes = b"package-t114".to_vec();
    let manifest = FlashPackageManifest {
        schema: PACKAGE_SCHEMA,
        package_id: "test.t114".into(),
        display_name: "Test T114".into(),
        version: "1".into(),
        publisher: "Test".into(),
        helpers: vec![crate::package::HelperRequirement {
            route: FlashRoute::AdafruitDfu,
            program: "adafruit-nrfutil".into(),
            version: "0.5.3.post16".into(),
            binary_sha256: None,
            artifacts: Vec::new(),
            license: "test".into(),
            source_url: "https://example.invalid/adafruit-nrfutil".into(),
            notice: "Test helper notice".into(),
        }],
        payload: Some(PackagePayload {
            path: "payload".into(),
            format: PayloadFormat::NrfDfuZip,
            byte_length: bytes.len() as u64,
            sha256: crate::package::sha256_hex(&bytes),
            write_bytes: bytes.len() as u64,
        }),
        parts: Vec::new(),
        targets: vec![PackageTarget {
            family: BoardFamily::T114,
            revision: "2.x".into(),
            processor: ProcessorKind::Nrf52840,
            flash_size: 1024 * 1024,
            bootloader: "s140-v6".into(),
            route: FlashRoute::AdafruitDfu,
        }],
        write_ranges: vec![FlashRange {
            start: 0x26000,
            length: bytes.len() as u32,
        }],
        preserved_ranges: vec![
            FlashRange {
                start: 0x26000 + bytes.len() as u32,
                length: 1,
            },
            NODE_TIMEBASE_PRESERVED_RANGE,
        ],
        regions: vec!["US915".into()],
        channel_capabilities: vec!["modem".into(), "node".into(), "rnode".into()],
        state_impact: StateImpact::Preserved,
        expected_application: ExpectedApplication {
            board: BoardFamily::T114,
            version: "0.0.1".into(),
            manual_check: None,
        },
        license: "MPL-2.0".into(),
        notices: "Notices".into(),
        source_revision: "test".into(),
        source_url: "https://example.invalid/source".into(),
        origin_url: "https://example.invalid/package".into(),
        publisher_signature: None,
        recovery: RecoveryInstructions {
            before_write: "Keep cable attached.".into(),
            after_failure: "Use DFU entry.".into(),
        },
        persistent_state: Some(PersistentStateCompatibility {
            schema: PERSISTENT_STATE_SCHEMA,
            native_node_guard: true,
            preserved_range: NODE_TIMEBASE_PRESERVED_RANGE,
        }),
    };
    FlashPackage::from_parts(manifest, "manifest", "payload", bytes).unwrap()
}

fn sparse_package() -> FlashPackage {
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
            program: "esptool".into(),
            version: "4.8.1".into(),
            binary_sha256: None,
            artifacts: Vec::new(),
            license: "GPL-2.0-or-later".into(),
            source_url: "https://example.invalid/esptool".into(),
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
            processor: ProcessorKind::Esp32S3,
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
            manual_check: None,
        },
        license: "MPL-2.0".into(),
        notices: "Notices".into(),
        source_revision: "test".into(),
        source_url: "https://example.invalid/source".into(),
        origin_url: "https://example.invalid/package".into(),
        publisher_signature: None,
        recovery: RecoveryInstructions {
            before_write: "Keep cable attached.".into(),
            after_failure: "Use ROM entry.".into(),
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

fn observation() -> DeviceObservation {
    DeviceObservation {
        transport: DeviceTransport::SerialPort("COM7".into()),
        status_reply: Some("tulle/heltec-v4 phy online".into()),
        hardware: HardwareFacts {
            processor: Some(ProcessorKind::Esp32S3),
            flash_size: Some(4 * 1024 * 1024),
            bootloader: Some("esp-rom".into()),
            loader_route: Some("esp-rom".into()),
            bootloader_usb: None,
        },
        selected_board: Some(BoardSelection::owner_confirmed(
            BoardFamily::HeltecV4,
            "4.2",
        )),
        firmware: FirmwareState::Retinue {
            family: BoardFamily::HeltecV4,
        },
        confidence: crate::device::EvidenceConfidence::OwnerConfirmed,
        contradictions: Vec::new(),
        native_node_state: NativeNodeState::Unknown,
    }
}

#[test]
fn compatible_observation_produces_an_explainable_plan() {
    let plan = plan_flash(&observation(), &package()).expect("facts are compatible");
    let platform = crate::package::helper_platform();
    assert_eq!(plan.route(), &FlashRoute::EspRom);
    assert_eq!(plan.helper(), "espflash");
    assert_eq!(
        plan.helper_identity().platform.as_deref(),
        Some(platform.as_str())
    );
    assert_eq!(
        plan.helper_identity().binary_sha256.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(
        plan.helper_identity().archive_sha256.as_deref(),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
    assert_eq!(plan.state_impact(), &StateImpact::Preserved);
    assert!(plan.describe().contains("recovery before write"));
    assert!(plan.describe().contains(&platform));
}

#[test]
fn armed_native_node_refuses_a_package_without_guard_support() {
    let mut observation = observation();
    observation.native_node_state = NativeNodeState::Armed;
    let refusal = plan_flash(&observation, &package())
        .expect_err("an armed node cannot be replaced by a legacy package");
    assert!(
        refusal
            .reasons
            .contains(&RefusalReason::PersistentStateCompatibilityRequired)
    );
}

#[test]
fn armed_native_node_accepts_a_package_that_preserves_the_guard_range() {
    let mut observation = observation();
    observation.selected_board = Some(BoardSelection::owner_confirmed(BoardFamily::T114, "2.x"));
    observation.firmware = FirmwareState::Retinue {
        family: BoardFamily::T114,
    };
    observation.hardware = HardwareFacts {
        processor: Some(ProcessorKind::Nrf52840),
        flash_size: Some(1024 * 1024),
        bootloader: Some("s140-v6".into()),
        loader_route: Some("serial-dfu".into()),
        bootloader_usb: None,
    };
    observation.native_node_state = NativeNodeState::Armed;
    let plan = plan_flash(&observation, &t114_package())
        .expect("the current T114 package carries the durable-state declaration");
    assert!(
        plan.compatibility()
            .iter()
            .any(|fact| fact.name == "package node-timebase support"
                && fact.value.contains("0xe8000..0xec000"))
    );
}

#[test]
fn unarmed_and_unknown_states_keep_legacy_packages_eligible() {
    for state in [NativeNodeState::Unknown, NativeNodeState::Unarmed] {
        let mut observation = observation();
        observation.native_node_state = state;
        plan_flash(&observation, &package())
            .expect("a non-armed observation does not assert durable-state continuity");
    }
}

#[test]
fn documented_product_profile_is_retained_as_revision_evidence() {
    let mut observation = observation();
    observation.selected_board = Some(BoardSelection::documented_product_profile(
        BoardFamily::HeltecV4,
        "4.2",
        "Meshnology N39 WiFi LoRa 32 V4 kit",
        "https://wiki.meshnology.com/N39/Meshnology%20N39/",
    ));

    let plan = plan_flash(&observation, &package())
        .expect("a documented exact product profile remains a compatible selection");
    assert!(plan.compatibility().iter().any(|fact| {
        fact.name == "board revision"
            && fact.value == "4.2"
            && fact.source.contains("Meshnology N39")
            && fact.source.contains("wiki.meshnology.com")
    }));
}

#[test]
fn sparse_package_plan_preserves_every_artifact_and_offset() {
    let plan = plan_flash(&observation(), &sparse_package()).expect("sparse package plans");
    assert_eq!(plan.helper(), "esptool");
    assert_eq!(plan.parts().len(), 3);
    assert_eq!(
        plan.parts()
            .iter()
            .map(|part| part.offset)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(0x8000), Some(0x10000)]
    );
    assert_eq!(plan.write_ranges().len(), 3);
    assert!(plan.describe().contains(&plan.parts()[2].sha256));
}

#[test]
fn running_retinue_identity_can_plan_without_reopening_the_loader() {
    let mut observation = observation();
    observation.hardware = HardwareFacts::default();
    let plan = plan_flash(&observation, &package())
        .expect("a running, self-identified Retinue board has a known route");
    assert!(plan.compatibility().iter().all(|fact| {
        fact.name == "board family"
            || fact.name == "board revision"
            || fact.name == "native-node persistent state"
            || fact.name == "package node-timebase support"
            || fact.source == "running Retinue identity; checked against package"
    }));
}

#[test]
fn captured_t114_loader_record_can_plan_a_silent_foreign_application() {
    let observation = DeviceObservation {
        transport: DeviceTransport::SerialPort("COM10".into()),
        status_reply: None,
        hardware: HardwareFacts {
            processor: Some(ProcessorKind::Nrf52840),
            flash_size: Some(1024 * 1024),
            bootloader: Some("s140-v6".into()),
            loader_route: Some("captured-t114-loader-snapshot".into()),
            bootloader_usb: None,
        },
        selected_board: Some(BoardSelection::owner_confirmed(BoardFamily::T114, "2.x")),
        firmware: FirmwareState::Unknown,
        confidence: crate::device::EvidenceConfidence::OwnerConfirmed,
        contradictions: Vec::new(),
        native_node_state: NativeNodeState::Unknown,
    };
    let plan = plan_flash(&observation, &t114_package())
        .expect("a captured loader record can support the owner-selected current T114");
    assert!(plan.compatibility().iter().any(|fact| {
        fact.name == "processor" && fact.source == "captured HT-n5262 UF2 and SoftDevice record"
    }));
    assert!(plan.compatibility().iter().any(|fact| {
        fact.name == "board family"
            && fact.source
                == "owner-selected current board, checked against captured HT-n5262 loader record"
    }));
}

#[test]
fn v4_package_is_refused_for_t114() {
    let mut observation = observation();
    observation.selected_board = Some(BoardSelection::owner_confirmed(BoardFamily::T114, "2.1"));
    observation.firmware = FirmwareState::Retinue {
        family: BoardFamily::T114,
    };
    let refusal = plan_flash(&observation, &package()).expect_err("wrong board must refuse");
    assert!(
        refusal
            .reasons
            .iter()
            .any(|reason| matches!(reason, RefusalReason::UnsupportedBoard(BoardFamily::T114)))
    );
}

#[test]
fn t114_package_is_refused_for_v4() {
    let refusal = plan_flash(&observation(), &t114_package())
        .expect_err("the T114 package must not plan for a V4");
    assert!(refusal.reasons.iter().any(|reason| matches!(
        reason,
        RefusalReason::UnsupportedBoard(BoardFamily::HeltecV4)
    )));
}

#[test]
fn conflicting_loader_evidence_is_refused() {
    let mut observation = observation();
    observation.hardware.processor = Some(ProcessorKind::Nrf52840);
    let refusal = plan_flash(&observation, &package()).expect_err("processor conflict must refuse");
    assert!(
        refusal
            .reasons
            .iter()
            .any(|reason| matches!(reason, RefusalReason::ProcessorConflict { .. }))
    );
}

#[test]
fn revision_and_confirmation_are_not_guessed() {
    let mut observation = observation();
    observation.selected_board = None;
    let refusal =
        plan_flash(&observation, &package()).expect_err("missing owner choice must refuse");
    assert!(
        refusal
            .reasons
            .iter()
            .any(|reason| matches!(reason, RefusalReason::BoardSelectionRequired))
    );

    observation.selected_board = Some(BoardSelection {
        family: BoardFamily::HeltecV4,
        revision: "4.1".into(),
        confirmed_by_owner: true,
        evidence: crate::device::BoardSelectionEvidence::CarrierMarking,
    });
    let refusal =
        plan_flash(&observation, &package()).expect_err("unsupported revision must refuse");
    assert!(
        refusal
            .reasons
            .iter()
            .any(|reason| matches!(reason, RefusalReason::UnsupportedRevision { .. }))
    );
}

#[test]
fn missing_loader_facts_are_refused_without_opening_a_port() {
    let mut observation = observation();
    observation.hardware = HardwareFacts::default();
    observation.firmware = FirmwareState::Bootloader;
    observation.status_reply = None;
    let refusal = plan_flash(&observation, &package()).expect_err("missing facts must refuse");
    assert!(
        refusal
            .reasons
            .iter()
            .any(|reason| matches!(reason, RefusalReason::ProcessorMissing))
    );
    assert!(
        refusal
            .reasons
            .iter()
            .any(|reason| matches!(reason, RefusalReason::FlashSizeMissing))
    );
    assert!(
        refusal
            .reasons
            .iter()
            .any(|reason| matches!(reason, RefusalReason::BootloaderMissing))
    );
}
