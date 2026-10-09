use linkboy::OwnerStage;

use super::*;

#[test]
fn installer_starts_with_the_first_owner_page() {
    let installer = FirmwareInstaller::new();
    let view = installer.view();
    assert_eq!(view.stage, OwnerStage::ChooseDevice);
    assert_eq!(view.title, "Choose device");
    assert_eq!(view.device, None);
    assert_eq!(view.review, None);
    assert_eq!(view.result, None);
}

#[test]
fn catalog_loads_and_resolves_verified_packages() {
    let index_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../firmware/packages/index.toml");
    let catalog = FirmwareCatalog::load(index_path).expect("package catalog should load");
    assert!(
        catalog.packages().len() >= 2,
        "the Retinue V4 and T114 packages remain present alongside any added packages"
    );
    assert_eq!(
        catalog
            .package("retinue.heltec-v4")
            .expect("V4 should be catalogued")
            .state,
        linkboy::CatalogState::ProvenRecipe
    );
    let package = catalog
        .load_package("retinue.t114")
        .expect("T114 manifest should resolve");
    assert_eq!(package.manifest().package_id, "retinue.t114");
}

#[test]
fn an_owner_confirmed_t114_dfu_port_keeps_loader_evidence_and_transport_state() {
    let snapshot = linkboy::T114LoaderSnapshot {
        schema: linkboy::discovery::T114_LOADER_SNAPSHOT_SCHEMA,
        model: "HT-n5262".into(),
        uf2_bootloader: "0.9.0".into(),
        softdevice: "S140 6.1.1".into(),
        processor: linkboy::ProcessorKind::Nrf52840,
        flash_size: 1024 * 1024,
    };

    let observation = observe_t114_serial_dfu_port("COM10", "2.x".into(), &snapshot);

    assert_eq!(
        observation.transport,
        linkboy::DeviceTransport::SerialDfuPort("COM10".into())
    );
    assert_eq!(
        observation.firmware,
        linkboy::device::FirmwareState::Bootloader
    );
    assert_eq!(
        observation.hardware.loader_route.as_deref(),
        Some("captured-t114-loader-snapshot")
    );
    assert_eq!(
        observation
            .selected_board
            .as_ref()
            .map(|board| &board.family),
        Some(&linkboy::BoardFamily::T114)
    );
}
