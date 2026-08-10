//! Replay of RNS 1.4.2's signed artifacts, captured from the shipped `rnid` executable.
//!
//! This is the evidence behind ASSURE3. The claim being tested is narrow and worth stating
//! exactly: given the same identity, message, and metadata, retinue emits the *same bytes*
//! RNS emits, and validates what RNS produced. Ed25519 is deterministic, so byte equality is
//! reachable and anything less would be a weaker claim dressed up as a passing test.
//!
//! Regenerate with
//! `oracle/.venv/Scripts/python.exe -u oracle/capture_signed_artifact.py`. The fixture is
//! committed, so this suite needs no Python.

use retinue::artifact::{self, Error};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::msgpack::Value;

/// Prns's published constant for the detached case, at pinned commit `72b6b30d`.
///
/// Cross-checking it here is the point: a donor's own tests cannot confirm a donor's own
/// vectors. RNS produced this string in our capture, independently, so the constant Prns
/// ships is now corroborated rather than merely asserted.
const PRNS_PUBLISHED_RSG: &str = concat!(
    "e44d954d391c2393bdd24ebcfb94ba12db4ea2fc6c0f37b34b4072d10657655a",
    "521037c83b09f96a894640e7d5d9796022be27f7e177201a2185a298b844e302",
    "83a86861736874797065a6736861323536a468617368c4200e231b72dd5437d4",
    "095002c7d07b34ff13571911857c83b1281e576cee65fa4ea46d65746182a673",
    "69676e6572c4104cd0cc45a7405dbd5cf9b5be1ef92f10a67075626b6579c440",
    "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20",
    "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737",
);

struct Case {
    name: String,
    secret: [u8; 64],
    message: Vec<u8>,
    embed: bool,
    metadata: Vec<(String, Value)>,
    artifact: Vec<u8>,
}

fn cases() -> Vec<Case> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rns_signed_artifact.json"
    );
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("missing signed-artifact fixture: {e}. Run oracle/capture_signed_artifact.py.")
    });
    let fixture: serde_json::Value = serde_json::from_str(&raw).expect("fixture is json");

    fixture["cases"]
        .as_array()
        .expect("cases array")
        .iter()
        .map(|case| {
            let mut secret = [0u8; 64];
            hex::decode_to_slice(case["secret_hex"].as_str().expect("secret"), &mut secret)
                .expect("valid hex secret");
            Case {
                name: case["name"].as_str().expect("name").to_string(),
                secret,
                message: case["message_utf8"]
                    .as_str()
                    .expect("message")
                    .as_bytes()
                    .to_vec(),
                embed: case["embed"].as_bool().expect("embed"),
                metadata: case["metadata"]
                    .as_array()
                    .expect("metadata array")
                    .iter()
                    .map(|entry| {
                        let key = entry["key"].as_str().expect("key").to_string();
                        let literal = entry["literal"].as_str().expect("literal");
                        (
                            key,
                            value_of(entry["type"].as_str().expect("type"), literal),
                        )
                    })
                    .collect(),
                artifact: hex::decode(case["artifact_hex"].as_str().expect("artifact"))
                    .expect("valid hex artifact"),
            }
        })
        .collect()
}

/// Rebuild the typed metadata value from the literal `rnid`'s configobj spec coerced.
fn value_of(kind: &str, literal: &str) -> Value {
    match kind {
        "str" => Value::Str(literal.to_string()),
        "uint" => Value::Uint(literal.parse().expect("integer literal")),
        "bool" => Value::Bool(literal == "True"),
        "str_list" => Value::Array(
            literal
                .split(',')
                .map(|item| Value::Str(item.trim().to_string()))
                .collect(),
        ),
        other => panic!("fixture uses an unmodelled metadata type: {other}"),
    }
}

#[test]
fn every_captured_artifact_is_reproduced_byte_for_byte() {
    let cases = cases();
    assert!(!cases.is_empty(), "fixture holds no cases");
    for case in &cases {
        let signer = PrivateIdentity::from_secret_bytes(&case.secret);
        let borrowed: Vec<(&str, Value)> = case
            .metadata
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect();
        let built = artifact::create(&signer, &case.message, case.embed, &borrowed)
            .unwrap_or_else(|e| panic!("{}: create failed: {e}", case.name));
        assert_eq!(
            hex::encode(&built),
            hex::encode(&case.artifact),
            "{} did not reproduce the captured bytes",
            case.name
        );
    }
}

#[test]
fn every_captured_artifact_validates_and_says_what_rns_put_in_it() {
    for case in &cases() {
        let signer = PrivateIdentity::from_secret_bytes(&case.secret);
        // A detached artifact needs its message supplied; an embedded one carries it.
        let supplied = (!case.embed).then_some(case.message.as_slice());
        let validated = artifact::validate(&case.artifact, supplied, Some(signer.hash()))
            .unwrap_or_else(|e| panic!("{}: validate failed: {e}", case.name));

        assert_eq!(validated.signer.hash(), signer.hash(), "{}", case.name);
        assert_eq!(
            validated.signer.to_public_bytes(),
            signer.public().to_public_bytes(),
            "{}",
            case.name
        );
        assert_eq!(validated.metadata, case.metadata, "{}", case.name);
        if case.embed {
            assert_eq!(
                validated.embedded_message.as_deref(),
                Some(case.message.as_slice()),
                "{}",
                case.name
            );
        } else {
            assert!(validated.embedded_message.is_none(), "{}", case.name);
        }
    }
}

#[test]
fn a_captured_artifact_refuses_the_wrong_message_and_the_wrong_signer() {
    // Same vectors, run backwards. Reproducing bytes proves the encoder; refusing these
    // proves the checks are load-bearing rather than decorative.
    for case in &cases() {
        let elsewhere = AddressHash::from_bytes([0x55; 16]);
        assert_eq!(
            artifact::validate(&case.artifact, None, Some(elsewhere)),
            Err(Error::UnexpectedSigner),
            "{}",
            case.name
        );
        assert_eq!(
            artifact::validate(&case.artifact, Some(b"not the message"), None),
            Err(Error::MessageMismatch),
            "{}",
            case.name
        );

        // Flip one bit of the signature. Everything else still parses, so this isolates the
        // signature check from the hash check.
        let mut forged = case.artifact.clone();
        forged[0] ^= 0x01;
        let supplied = (!case.embed).then_some(case.message.as_slice());
        assert_eq!(
            artifact::validate(&forged, supplied, None),
            Err(Error::BadSignature),
            "{}",
            case.name
        );
    }
}

#[test]
fn rns_corroborates_the_vector_prns_publishes() {
    let case = cases()
        .into_iter()
        .find(|case| case.name == "rsg_prns_identity")
        .expect("fixture holds the prns-identity detached case");
    assert_eq!(
        hex::encode(&case.artifact),
        PRNS_PUBLISHED_RSG,
        "RNS 1.4.2 no longer emits the artifact Prns's tests assert"
    );
}
