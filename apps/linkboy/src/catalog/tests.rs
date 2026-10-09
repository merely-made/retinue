use super::*;

fn package(state: CatalogState) -> CatalogPackage {
    CatalogPackage {
        package_id: "retinue.test".into(),
        release_sequence: 1,
        manifest: "test.toml".into(),
        firmware_publisher: "Merely Made".into(),
        state,
        instructions_url: "https://example.com/install".into(),
        recovery_url: "https://example.com/recover".into(),
        installer_receipts: Vec::new(),
        recovery_receipts: Vec::new(),
        receipt_hosts: Vec::new(),
        purchase_url: None,
    }
}

fn index(package: CatalogPackage) -> PackageIndex {
    PackageIndex {
        schema: PACKAGE_INDEX_SCHEMA.into(),
        publisher: "Merely Made".into(),
        version: "1".into(),
        packages: vec![package],
        publisher_signature: None,
    }
}

const TEST_PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1556193335\tfile:test
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

#[test]
fn partial_catalog_is_valid_without_receipts() {
    assert!(index(package(CatalogState::Partial)).validate().is_ok());
}

#[test]
fn proven_recipe_requires_both_receipt_kinds() {
    let mut value = package(CatalogState::ProvenRecipe);
    value
        .installer_receipts
        .push("https://example.com/install-receipt".into());
    assert!(index(value).validate().is_err());
}

#[test]
fn sellable_catalog_requires_purchase_link_and_receipts() {
    let mut value = package(CatalogState::Sellable);
    value
        .installer_receipts
        .push("https://example.com/install-receipt".into());
    value
        .recovery_receipts
        .push("https://example.com/recovery-receipt".into());
    value.receipt_hosts.push("windows-x86_64".into());
    assert!(index(value.clone()).validate().is_err());
    value.purchase_url = Some("https://example.com/buy".into());
    assert!(index(value).validate().is_ok());
}

#[test]
fn retained_public_index_promotes_only_receipted_packages() {
    let index: PackageIndex =
        toml::from_str(include_str!("../../../../firmware/packages/index.toml"))
            .expect("parse retained public package index");
    index
        .validate()
        .expect("validate retained public package index");

    let package = |package_id| {
        index
            .packages
            .iter()
            .find(|package| package.package_id == package_id)
            .expect("named package")
    };
    assert_eq!(
        package("retinue.heltec-v4").state,
        CatalogState::ProvenRecipe
    );
    assert_eq!(
        package("retinue.heltec-v4").receipt_hosts,
        ["windows-x86_64"]
    );
    assert_eq!(
        package("retinue.heltec-v4-resident").state,
        CatalogState::Partial
    );
    assert_eq!(package("retinue.t114").state, CatalogState::Partial);
    assert_eq!(
        package("meshtastic.heltec-mesh-node-t114").state,
        CatalogState::Partial
    );
    assert!(
        package("meshtastic.heltec-mesh-node-t114")
            .installer_receipts
            .is_empty()
    );
    assert!(
        !index
            .packages
            .iter()
            .any(|p| p.package_id == "prns.hopspot.heltec-v4")
    );
    let staging: PackageIndex = toml::from_str(include_str!(
        "../../../../firmware/packages/windows-v4-staging-index.toml"
    ))
    .unwrap();
    staging.validate().unwrap();
    assert!(
        !staging
            .packages
            .iter()
            .any(|p| p.package_id == "prns.hopspot.heltec-v4")
    );
}

#[test]
fn minisign_verifier_accepts_the_retained_vector_and_rejects_changed_bytes() {
    verify_minisign(b"test", TEST_PUBLIC_KEY, TEST_SIGNATURE).unwrap();
    assert!(verify_minisign(b"Test", TEST_PUBLIC_KEY, TEST_SIGNATURE).is_err());
}

#[test]
fn signing_bytes_exclude_the_signature_and_are_domain_separated() {
    let mut index = index(package(CatalogState::Partial));
    let unsigned = index.signing_bytes().unwrap();
    index.publisher_signature = Some(CatalogSignature {
        format: CatalogSignatureFormat::Minisign,
        key_id: "test-key".into(),
        signature: TEST_SIGNATURE.into(),
    });
    assert_eq!(index.signing_bytes().unwrap(), unsigned);
    assert!(unsigned.starts_with(CATALOG_SIGNING_DOMAIN.as_bytes()));
}

#[test]
fn parsed_catalog_is_not_authenticated_without_a_signature_or_local_trust() {
    let trust = CatalogTrust {
        schema: CATALOG_TRUST_SCHEMA.into(),
        keys: vec![TrustedCatalogKey {
            publisher: "Merely Made".into(),
            key_id: "test-key".into(),
            public_key: TEST_PUBLIC_KEY.into(),
        }],
    };
    assert!(matches!(
        index(package(CatalogState::Partial)).authenticate(&trust),
        Err(CatalogError::MissingSignature)
    ));

    let mut signed = index(package(CatalogState::Partial));
    signed.publisher_signature = Some(CatalogSignature {
        format: CatalogSignatureFormat::Minisign,
        key_id: "other-key".into(),
        signature: TEST_SIGNATURE.into(),
    });
    assert!(matches!(
        signed.authenticate(&trust),
        Err(CatalogError::UntrustedPublisher { .. })
    ));
}
