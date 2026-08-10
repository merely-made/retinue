//! Retained publisher evidence for the first upstream V4 package.
//!
//! This does not make the Prns key a Linkboy trust root. It proves only that the release source
//! recorded beside the package was signed by the key named in the package evidence.

use std::fs;
use std::path::PathBuf;

use minisign_verify::{PublicKey, Signature};

const PRNS_RELEASE_KEY: &str = "RWQagUCx2MT1G8GWiTmVGnBTDlIbJketjPupSYmBpT8fH9xZRGfsW1a7";

fn release_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../firmware/packages/hopspot-v4-0.3.4")
        .join(name)
}

fn verify(name: &str) {
    let key = PublicKey::from_base64(PRNS_RELEASE_KEY).expect("retained Prns public key");
    let bytes = fs::read(release_path(name)).expect("retained signed artifact");
    let signature = Signature::decode(
        &fs::read_to_string(release_path(&format!("{name}.minisig")))
            .expect("retained Minisign signature"),
    )
    .expect("valid Minisign syntax");
    key.verify(&bytes, &signature, false)
        .expect("Prns signature must verify retained source evidence");
}

#[test]
fn channel_descriptor_and_flash_manifest_are_signed_by_the_recorded_prns_key() {
    verify("stable.json");
    verify("flash-manifest.json");
}
