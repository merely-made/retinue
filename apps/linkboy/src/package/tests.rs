use super::*;

fn manifest(payload: &[u8]) -> FlashPackageManifest {
    FlashPackageManifest {
        schema: PACKAGE_SCHEMA,
        package_id: "test.package".into(),
        display_name: "Test package".into(),
        version: "1".into(),
        publisher: "Test publisher".into(),
        helpers: vec![HelperRequirement {
            route: FlashRoute::EspRom,
            program: "espflash".into(),
            version: "4.5.0".into(),
            binary_sha256: None,
            artifacts: Vec::new(),
            license: "MIT OR Apache-2.0".into(),
            source_url: "https://example.invalid/espflash".into(),
            notice: "Test helper notice".into(),
        }],
        payload: Some(PackagePayload {
            path: "payload.bin".into(),
            format: PayloadFormat::EspflashElf,
            byte_length: payload.len() as u64,
            sha256: sha256_hex(payload),
            write_bytes: payload.len() as u64,
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
        notices: "Test notices".into(),
        source_revision: "test".into(),
        source_url: "https://example.invalid/source".into(),
        origin_url: "https://example.invalid/package".into(),
        publisher_signature: None,
        recovery: RecoveryInstructions {
            before_write: "Keep the cable connected.".into(),
            after_failure: "Enter the ROM loader again.".into(),
        },
        persistent_state: None,
    }
}

fn sparse_manifest(parts: &[(&str, FirmwarePartKind, u32, &[u8])]) -> FlashPackageManifest {
    let mut value = manifest(b"legacy-container");
    value.payload = None;
    value.write_ranges.clear();
    value.preserved_ranges = vec![FlashRange {
        start: 0xd000,
        length: 0x1000,
    }];
    value.parts = parts
        .iter()
        .map(|(path, kind, offset, bytes)| PackagePart {
            kind: kind.clone(),
            path: (*path).into(),
            format: PayloadFormat::RawBinary,
            offset: Some(*offset),
            byte_length: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            write_bytes: bytes.len() as u64,
        })
        .collect();
    value.publisher_signature = Some(PublisherSignature {
        format: PublisherSignatureFormat::Minisign,
        key_id: "1FB2CA18B2C25E1F".into(),
        signed_manifest_url: "https://example.invalid/hopspot/flash-manifest.json".into(),
        signed_manifest_sha256: "b".repeat(64),
        signature: "untrusted comment: retained upstream signature".into(),
    });
    value
}

#[test]
fn hashes_are_stable() {
    assert_eq!(
        sha256_hex(b"linkboy"),
        "5e27c306d7ec7d9f0527bbd3a7591e1578d82849c23cdd0a373a7f69b89e4e95"
    );
}

#[test]
fn a_single_changed_byte_is_rejected() {
    let original = b"payload".to_vec();
    let mut changed = original.clone();
    changed[0] ^= 1;
    let error =
        FlashPackage::from_parts(manifest(&original), "manifest.toml", "payload.bin", changed)
            .expect_err("changing one payload byte must invalidate the package");
    assert!(matches!(error, PackageError::HashMismatch { .. }));
}

#[test]
fn protected_ranges_are_rejected_before_payload_use() {
    let mut value = manifest(b"payload");
    value.preserved_ranges[0].start = 0x3eff00;
    let error =
        FlashPackage::from_parts(value, "manifest.toml", "payload.bin", b"payload".to_vec())
            .expect_err("a write crossing a preserved range must be refused");
    assert!(matches!(error, PackageError::ProtectedRangeOverlap));
}

#[test]
fn native_node_guard_must_match_a_real_preserved_range() {
    let mut value = manifest(b"payload");
    value.persistent_state = Some(PersistentStateCompatibility {
        schema: PERSISTENT_STATE_SCHEMA,
        native_node_guard: true,
        preserved_range: NODE_TIMEBASE_PRESERVED_RANGE,
    });
    let error =
        FlashPackage::from_parts(value, "manifest.toml", "payload.bin", b"payload".to_vec())
            .expect_err("a guard claim without a preserved range must be refused");
    assert!(
        matches!(error, PackageError::InvalidField(message) if message.contains("preserved_ranges"))
    );
}

#[test]
fn sparse_parts_keep_ordered_hashes_offsets_and_publisher_evidence() {
    let bootloader = b"bootloader";
    let partition_table = b"partition-table";
    let application = b"application";
    let manifest = sparse_manifest(&[
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
    ]);
    let package = FlashPackage::from_verified_parts(
        manifest,
        "manifest.toml",
        vec![
            (PathBuf::from("bootloader.bin"), bootloader.to_vec()),
            (
                PathBuf::from("partition-table.bin"),
                partition_table.to_vec(),
            ),
            (PathBuf::from("application.bin"), application.to_vec()),
        ],
    )
    .expect("ordered sparse parts should verify");
    assert_eq!(package.parts().len(), 3);
    assert_eq!(
        package
            .parts()
            .iter()
            .map(|part| part.declaration().offset)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(0x8000), Some(0x10000)]
    );
    assert_eq!(package.manifest().write_ranges().len(), 3);
    assert!(package.manifest().publisher_signature.is_some());
}

#[test]
fn changed_sparse_part_is_rejected_before_a_plan_exists() {
    let bootloader = b"bootloader";
    let partition_table = b"partition-table";
    let application = b"application";
    let manifest = sparse_manifest(&[
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
    ]);
    let error = FlashPackage::from_verified_parts(
        manifest,
        "manifest.toml",
        vec![
            (PathBuf::from("bootloader.bin"), bootloader.to_vec()),
            (PathBuf::from("partition-table.bin"), b"changed".to_vec()),
            (PathBuf::from("application.bin"), application.to_vec()),
        ],
    )
    .expect_err("a changed sparse artifact must invalidate the complete package");
    assert!(matches!(error, PackageError::LengthMismatch { .. }));
}

#[test]
fn manual_external_package_may_leave_retinue_capabilities_unspecified() {
    let bootloader = b"bootloader";
    let partition_table = b"partition-table";
    let application = b"application";
    let mut manifest = sparse_manifest(&[
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
    ]);
    manifest.regions.clear();
    manifest.channel_capabilities.clear();
    manifest.expected_application.manual_check = Some("Exercise the upstream interface.".into());
    assert!(
        FlashPackage::from_verified_parts(
            manifest,
            "manifest",
            vec![
                ("bootloader.bin".into(), bootloader.to_vec()),
                ("partition-table.bin".into(), partition_table.to_vec()),
                ("application.bin".into(), application.to_vec()),
            ],
        )
        .is_ok()
    );
}

#[test]
fn sparse_part_cannot_enter_a_preserved_provisioning_slot() {
    let bootloader = b"bootloader";
    let partition_table = b"partition-table";
    let application = b"application";
    let manifest = sparse_manifest(&[
        (
            "bootloader.bin",
            FirmwarePartKind::Bootloader,
            0,
            bootloader,
        ),
        (
            "partition-table.bin",
            FirmwarePartKind::PartitionTable,
            0xd000,
            partition_table,
        ),
        (
            "application.bin",
            FirmwarePartKind::Application,
            0x10000,
            application,
        ),
    ]);
    let error = FlashPackage::from_verified_parts(
        manifest,
        "manifest.toml",
        vec![
            (PathBuf::from("bootloader.bin"), bootloader.to_vec()),
            (
                PathBuf::from("partition-table.bin"),
                partition_table.to_vec(),
            ),
            (PathBuf::from("application.bin"), application.to_vec()),
        ],
    )
    .expect_err("a sparse write cannot touch provisioning");
    assert!(matches!(error, PackageError::ProtectedRangeOverlap));
}

#[test]
fn unknown_manifest_keys_are_rejected() {
    let text = r#"
schema = 1
package_id = "x"
display_name = "x"
version = "1"
publisher = "x"
unexpected = true
"#;
    assert!(toml::from_str::<FlashPackageManifest>(text).is_err());
}

#[test]
fn nrf52840_uf2_without_the_matching_family_id_is_rejected() {
    let mut bytes =
        crate::uf2::encode_application(b"application", 0x26000, crate::uf2::NRF52840_FAMILY_ID)
            .unwrap();
    bytes[28..32].copy_from_slice(&0_u32.to_le_bytes());

    let mut value = manifest(&bytes);
    value.helpers[0].route = FlashRoute::Uf2MassStorage;
    value.helpers[0].program = FlashRoute::Uf2MassStorage.helper().into();
    value.payload = Some(PackagePayload {
        path: "payload.uf2".into(),
        format: PayloadFormat::Uf2,
        byte_length: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
        write_bytes: crate::uf2::PAYLOAD_SIZE as u64,
    });
    value.targets = vec![PackageTarget {
        family: BoardFamily::T114,
        revision: "2.x".into(),
        processor: ProcessorKind::Nrf52840,
        flash_size: 1024 * 1024,
        bootloader: "adafruit-uf2-0.9.0".into(),
        route: FlashRoute::Uf2MassStorage,
    }];
    value.write_ranges = vec![FlashRange {
        start: 0x26000,
        length: crate::uf2::PAYLOAD_SIZE as u32,
    }];
    value.preserved_ranges = vec![FlashRange {
        start: 0x26100,
        length: 1,
    }];
    value.expected_application.board = BoardFamily::T114;

    let error = FlashPackage::from_parts(value, "manifest", "payload.uf2", bytes)
        .expect_err("the nRF52840 family guard must be part of package admission");
    assert!(matches!(
        error,
        PackageError::InvalidField(detail) if detail.contains("nRF52840 family id")
    ));
}
