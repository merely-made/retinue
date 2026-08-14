//! Retained official Meshtastic T114 release evidence.

use std::path::PathBuf;

use linkboy::{FlashPackage, FlashRoute};

fn package_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../firmware/packages/meshtastic-t114-2.7.26.54e0d8d.toml")
}

#[test]
fn official_t114_uf2_has_the_admitted_hash_and_address_map() {
    let package = FlashPackage::load(package_path()).expect("retained Meshtastic package");
    assert_eq!(
        package.manifest().source_revision,
        "54e0d8d0ab2ff56b3a9ce967e53f79e49af560fb"
    );
    assert_eq!(
        package.manifest().source_url,
        "https://github.com/meshtastic/firmware/tree/54e0d8d0ab2ff56b3a9ce967e53f79e49af560fb"
    );
    assert_eq!(
        package.manifest().targets[0].route,
        FlashRoute::Uf2MassStorage
    );
    assert_eq!(package.manifest().write_ranges()[0].start, 0x26000);
    assert_eq!(package.manifest().write_ranges()[0].length, 0xb3200);
    assert_eq!(package.parts()[0].declaration().write_bytes, 733696);
    assert_eq!(
        package.parts()[0].declaration().sha256,
        "e038b1067b6fa12ed77853337e0e416425f30a7589a907f8ac1600f21b8d02b9"
    );
}
