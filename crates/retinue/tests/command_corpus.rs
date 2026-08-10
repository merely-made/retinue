//! The committed fuzz seeds for the command verifier, checked for the property that makes
//! them worth committing.
//!
//! A seed corpus rots quietly. If the envelope layout changes and the seeds are not
//! regenerated, `cargo fuzz` still runs, still reports coverage, and still finds nothing,
//! because every seed now dies at the first length check instead of reaching the counter
//! window and the signature. Nothing fails; the campaign just stops testing anything.
//!
//! So the seeds are asserted here, in the ordinary test run, where a break is loud.

use retinue::command::{Refusal, Verifier};
use retinue::hash::AddressHash;
use retinue::identity::{IDENTITY_LEN, PrivateIdentity};

/// The node the fuzz target verifies for. Must match `fuzz/fuzz_targets/retinue_command_accept.rs`.
const NODE: AddressHash = AddressHash::from_bytes([0x0a; 16]);

fn operator(fill: u8) -> PrivateIdentity {
    let mut secret = [0u8; IDENTITY_LEN];
    secret[..32].fill(fill);
    secret[32..].fill(fill ^ 0xff);
    PrivateIdentity::from_secret_bytes(&secret)
}

fn seeds() -> Vec<(String, Vec<u8>)> {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fuzz/seeds/retinue-command-accept"
    );
    let mut found: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read the seed corpus at {dir}: {e}"))
        .map(|entry| entry.expect("readable dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "seed"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("named file")
                .to_string_lossy()
                .into_owned();
            (name, std::fs::read(&path).expect("readable seed"))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[test]
fn every_seed_still_reaches_the_verifier_and_is_accepted() {
    let seeds = seeds();
    assert!(!seeds.is_empty(), "the command fuzz corpus is empty");

    for (name, seed) in seeds {
        // The fuzzer's input is `selector || body`; selector 0 hands the body to the
        // verifier untouched, which is the shape every committed seed uses.
        let (selector, body) = seed.split_first().expect("seed carries a selector byte");
        assert_eq!(
            *selector, 0,
            "{name}: seed does not select the raw-bytes branch"
        );

        let trusted = operator(0x11);
        let mut verifier: Verifier<4> = Verifier::new(NODE);
        verifier
            .authorize(*trusted.public())
            .expect("allowlist has room");

        let accepted = verifier
            .accept(body)
            .unwrap_or_else(|e| panic!("{name}: seed no longer verifies ({e:?}). Regenerate it."));
        assert_eq!(accepted.key_id, trusted.hash(), "{name}");

        // And the replay property the fuzz target asserts, checked once here so a failure
        // during a campaign is not the first time anyone sees it.
        assert_eq!(
            verifier.accept(body).err(),
            Some(Refusal::CounterReplayed),
            "{name}"
        );
    }
}
