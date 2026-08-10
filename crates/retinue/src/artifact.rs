//! Signed artifacts: RNS's detached signature (`.rsg`) and signed message (`.rsm`).
//!
//! An artifact binds a message to an identity without a link, a session, or a live peer. It
//! is what `rnid -s` and `rnid -S` write, and it is the interoperable way to hand someone a
//! service description, an invitation, a distribution record, or a firmware manifest and let
//! them check who stands behind it.
//!
//! ```text
//! artifact = ed25519_signature(64) || envelope
//! envelope = msgpack {
//!     "hashtype": "sha256",
//!     "hash":     sha256(message),          // 32 bytes
//!     "meta":     { "signer": identity_hash(16), "pubkey": public_identity(64), .. },
//!     "message":  message,                  // present in an .rsm, absent in an .rsg
//! }
//! ```
//!
//! The signature covers the encoded envelope, not the message: the message is reached
//! through the hash the envelope commits to. An `.rsg` therefore travels beside the file it
//! signs, and an `.rsm` carries its own.
//!
//! # What this is not
//!
//! A carrier, not an authorization policy. Verifying an artifact tells you which identity
//! signed which bytes. It says nothing about whether that identity may command this node,
//! whether the message is fresh, or whether it has been seen before. Those are
//! [`crate::command`]'s job, and deliberately not this module's; see
//! `design_docs/2026-08-10_fs2_command_carrier_decision.md`.
//!
//! # Provenance
//!
//! The envelope layout was read from
//! [Prns](https://github.com/KenAKAFrosty/Prns) (`prns-core/src/identity/signed_artifact.rs`,
//! Copyright (c) 2026 The Prns Authors, MIT OR Apache-2.0). The *vectors* this module is
//! tested against are not: they were produced by running RNS 1.4.2's own `rnid` executable
//! as a black box, which keeps them independent oracle evidence rather than
//! donor-conformance evidence. See `crates/retinue/oracle/capture_signed_artifact.py` and
//! `tests/fixtures/rns_signed_artifact.json`.

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::hash::{ADDRESS_HASH_LEN, AddressHash, full_hash};
use crate::identity::{IDENTITY_LEN, Identity, PrivateIdentity, SIGNATURE_LEN};
use crate::msgpack::{self, Value};

/// The two metadata keys the envelope owns. An operator cannot set them, because forging
/// them is exactly how a signer would claim to be someone else.
const RESERVED_KEYS: [&str; 2] = ["signer", "pubkey"];

/// The largest artifact this module will parse.
///
/// RNS itself is bounded only by memory. A node that accepts artifacts over the air needs a
/// number, and 64 KiB is far above any plausible manifest while staying inside what a host
/// can hold without thought. Nothing on the constrained tier accepts artifacts at all.
pub const MAX_ARTIFACT_LEN: usize = 64 * 1024;

/// What a validated artifact turned out to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    /// The identity that signed, recovered from the envelope's own `pubkey` field and
    /// checked against the `signer` hash beside it.
    pub signer: Identity,
    /// Operator metadata, with the two reserved keys removed. Order is as signed.
    pub metadata: Vec<(String, Value)>,
    /// The message, when the artifact carried one (an `.rsm`).
    pub embedded_message: Option<Vec<u8>>,
}

/// Why an artifact did not validate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Shorter than a bare signature, so there is no envelope at all.
    TooShort,
    /// Larger than [`MAX_ARTIFACT_LEN`].
    TooLarge,
    /// The envelope is not the map this format defines.
    BadEnvelope(msgpack::Error),
    /// A required field was missing, or held the wrong type.
    MissingField,
    /// `hashtype` named something other than `sha256`.
    UnsupportedHash,
    /// The `pubkey` field is not a valid identity.
    BadKey,
    /// `pubkey` does not hash to the `signer` beside it: the envelope contradicts itself.
    SignerMismatch,
    /// The signer is not the identity the caller required.
    UnexpectedSigner,
    /// An `.rsg` was validated without supplying the message it signs.
    MessageRequired,
    /// The message does not hash to the value the envelope committed to.
    MessageMismatch,
    /// The Ed25519 signature over the envelope did not verify.
    BadSignature,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            Self::TooShort => "artifact is shorter than a signature",
            Self::TooLarge => "artifact exceeds the size bound",
            Self::BadEnvelope(_) => "envelope is not well-formed MessagePack",
            Self::MissingField => "envelope is missing a required field",
            Self::UnsupportedHash => "envelope names a hash other than sha256",
            Self::BadKey => "envelope carries an invalid public key",
            Self::SignerMismatch => "envelope's public key does not match its signer hash",
            Self::UnexpectedSigner => "artifact was signed by a different identity",
            Self::MessageRequired => "detached artifact needs the message it signs",
            Self::MessageMismatch => "message does not match the envelope's hash",
            Self::BadSignature => "signature did not verify",
        };
        f.write_str(text)
    }
}

/// Build a signed artifact over `message`.
///
/// With `embed` set the message travels inside the artifact (an `.rsm`); without it the
/// artifact is a detached signature (an `.rsg`) and the verifier must be handed the message
/// separately. Reserved metadata keys supplied by the caller are dropped rather than
/// honoured: the envelope names its own signer.
pub fn create(
    signer: &PrivateIdentity,
    message: &[u8],
    embed: bool,
    metadata: &[(&str, Value)],
) -> Result<Vec<u8>, Error> {
    let mut meta = Vec::with_capacity(metadata.len() + RESERVED_KEYS.len());
    meta.push((
        Value::Str("signer".to_string()),
        Value::Bin(signer.hash().as_slice().to_vec()),
    ));
    meta.push((
        Value::Str("pubkey".to_string()),
        Value::Bin(signer.public().to_public_bytes().to_vec()),
    ));
    for (key, value) in metadata {
        if !RESERVED_KEYS.contains(key) {
            meta.push((Value::Str((*key).to_string()), value.clone()));
        }
    }

    let mut fields = Vec::with_capacity(4);
    fields.push((
        Value::Str("hashtype".to_string()),
        Value::Str("sha256".to_string()),
    ));
    fields.push((
        Value::Str("hash".to_string()),
        Value::Bin(full_hash(message).to_vec()),
    ));
    fields.push((Value::Str("meta".to_string()), Value::Map(meta)));
    if embed {
        fields.push((
            Value::Str("message".to_string()),
            Value::Bin(message.to_vec()),
        ));
    }

    let envelope = msgpack::encode(&Value::Map(fields));
    if envelope.len() + SIGNATURE_LEN > MAX_ARTIFACT_LEN {
        return Err(Error::TooLarge);
    }

    let signature = signer.sign(&envelope);
    let mut artifact = Vec::with_capacity(SIGNATURE_LEN + envelope.len());
    artifact.extend_from_slice(&signature);
    artifact.extend_from_slice(&envelope);
    Ok(artifact)
}

/// Validate an artifact, returning what it says only if every check passes.
///
/// `message` supplies the bytes for a detached artifact; for an embedded one it is optional,
/// and when both are present the caller's copy is the one checked, so a mismatched pair is
/// caught rather than silently resolved in the artifact's favour. `required_signer`, when
/// given, is checked before the signature: an artifact from the wrong identity is refused as
/// such rather than as a bad signature.
pub fn validate(
    artifact: &[u8],
    message: Option<&[u8]>,
    required_signer: Option<AddressHash>,
) -> Result<Validated, Error> {
    if artifact.len() <= SIGNATURE_LEN {
        return Err(Error::TooShort);
    }
    if artifact.len() > MAX_ARTIFACT_LEN {
        return Err(Error::TooLarge);
    }
    let (signature, envelope) = artifact.split_at(SIGNATURE_LEN);
    let signature: [u8; SIGNATURE_LEN] = signature.try_into().expect("split at SIGNATURE_LEN");

    let decoded = msgpack::decode(envelope).map_err(Error::BadEnvelope)?;

    if decoded
        .get("hashtype")
        .and_then(Value::as_str)
        .ok_or(Error::MissingField)?
        != "sha256"
    {
        return Err(Error::UnsupportedHash);
    }
    let committed = decoded
        .get("hash")
        .and_then(Value::as_bin)
        .ok_or(Error::MissingField)?;

    let meta = decoded.get("meta").ok_or(Error::MissingField)?;
    let claimed_signer = meta
        .get("signer")
        .and_then(Value::as_bin)
        .and_then(AddressHash::from_slice)
        .filter(|_| {
            meta.get("signer").and_then(Value::as_bin).map(<[u8]>::len) == Some(ADDRESS_HASH_LEN)
        })
        .ok_or(Error::MissingField)?;
    let public: [u8; IDENTITY_LEN] = meta
        .get("pubkey")
        .and_then(Value::as_bin)
        .ok_or(Error::MissingField)?
        .try_into()
        .map_err(|_| Error::BadKey)?;
    let signer = Identity::from_public_bytes(&public).map_err(|_| Error::BadKey)?;

    // The envelope states the signer twice. If the two disagree, the artifact is lying about
    // one of them, and which one hardly matters.
    if signer.hash() != claimed_signer {
        return Err(Error::SignerMismatch);
    }
    if required_signer.is_some_and(|required| required != signer.hash()) {
        return Err(Error::UnexpectedSigner);
    }

    let embedded_message = decoded
        .get("message")
        .map(|value| {
            value
                .as_bin()
                .ok_or(Error::MissingField)
                .map(<[u8]>::to_vec)
        })
        .transpose()?;
    let bound = message
        .or(embedded_message.as_deref())
        .ok_or(Error::MessageRequired)?;
    if full_hash(bound) != committed {
        return Err(Error::MessageMismatch);
    }

    if !signer.verify(envelope, &signature) {
        return Err(Error::BadSignature);
    }

    let mut metadata = Vec::new();
    for (key, value) in meta.as_map().ok_or(Error::MissingField)? {
        let key = key.as_str().ok_or(Error::MissingField)?;
        if !RESERVED_KEYS.contains(&key) {
            metadata.push((key.to_string(), value.clone()));
        }
    }

    Ok(Validated {
        signer,
        metadata,
        embedded_message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// The identity the oracle capture uses. Fixed so the vectors are reproducible.
    fn oracle_identity() -> PrivateIdentity {
        let mut secret = [0u8; IDENTITY_LEN];
        secret[..32].fill(0x22);
        secret[32..].fill(0x11);
        PrivateIdentity::from_secret_bytes(&secret)
    }

    #[test]
    fn a_detached_artifact_round_trips() {
        let signer = oracle_identity();
        let artifact = create(&signer, b"payload", false, &[]).unwrap();
        let validated = validate(&artifact, Some(b"payload"), None).unwrap();
        assert_eq!(validated.signer.hash(), signer.hash());
        assert!(validated.embedded_message.is_none());
        assert!(validated.metadata.is_empty());
        // Detached means detached: without the message there is nothing to check the hash
        // against, and guessing is not an option.
        assert_eq!(validate(&artifact, None, None), Err(Error::MessageRequired));
    }

    #[test]
    fn an_embedded_artifact_carries_its_message_and_metadata() {
        let signer = oracle_identity();
        let metadata = [
            ("name", Value::Str("retinue".to_string())),
            ("version", Value::Uint(2)),
        ];
        let artifact = create(&signer, b"payload", true, &metadata).unwrap();
        let validated = validate(&artifact, None, None).unwrap();
        assert_eq!(
            validated.embedded_message.as_deref(),
            Some(b"payload".as_slice())
        );
        assert_eq!(
            validated.metadata,
            vec![
                ("name".to_string(), Value::Str("retinue".to_string())),
                ("version".to_string(), Value::Uint(2)),
            ]
        );
    }

    #[test]
    fn reserved_metadata_keys_cannot_be_supplied_by_the_caller() {
        let signer = oracle_identity();
        // A caller trying to name a different signer gets their key dropped, not honoured.
        let artifact = create(
            &signer,
            b"payload",
            true,
            &[("signer", Value::Bin(vec![0x55; ADDRESS_HASH_LEN]))],
        )
        .unwrap();
        let validated = validate(&artifact, None, None).unwrap();
        assert_eq!(validated.signer.hash(), signer.hash());
        assert!(validated.metadata.is_empty());
    }

    #[test]
    fn a_required_signer_is_enforced() {
        let signer = oracle_identity();
        let artifact = create(&signer, b"payload", true, &[]).unwrap();
        assert!(validate(&artifact, None, Some(signer.hash())).is_ok());
        assert_eq!(
            validate(
                &artifact,
                None,
                Some(AddressHash::from_bytes([0x55; ADDRESS_HASH_LEN]))
            ),
            Err(Error::UnexpectedSigner)
        );
    }

    #[test]
    fn the_wrong_message_is_refused() {
        let signer = oracle_identity();
        let artifact = create(&signer, b"payload", false, &[]).unwrap();
        assert_eq!(
            validate(&artifact, Some(b"other"), None),
            Err(Error::MessageMismatch)
        );
    }

    #[test]
    fn a_supplied_message_outranks_the_embedded_one() {
        // An .rsm that also gets an explicit message must agree with it. Preferring the
        // embedded copy here would let a caller believe it had checked its own bytes.
        let signer = oracle_identity();
        let artifact = create(&signer, b"payload", true, &[]).unwrap();
        assert_eq!(
            validate(&artifact, Some(b"different"), None),
            Err(Error::MessageMismatch)
        );
    }

    #[test]
    fn tampering_with_the_envelope_breaks_the_signature() {
        let signer = oracle_identity();
        let mut artifact = create(&signer, b"payload", true, &[("n", Value::Uint(1))]).unwrap();
        // Flip a bit in the metadata value, well past the signature and the message hash.
        let last = artifact.len() - 1;
        artifact[last] ^= 0x01;
        assert!(matches!(
            validate(&artifact, None, None),
            Err(Error::MessageMismatch | Error::BadSignature)
        ));
    }

    #[test]
    fn an_envelope_whose_two_signer_fields_disagree_is_refused() {
        let signer = oracle_identity();
        let artifact = create(&signer, b"payload", true, &[]).unwrap();
        let (signature, envelope) = artifact.split_at(SIGNATURE_LEN);
        let Value::Map(mut fields) = msgpack::decode(envelope).unwrap() else {
            panic!("envelope is a map");
        };
        for (key, value) in &mut fields {
            if key.as_str() == Some("meta")
                && let Value::Map(meta) = value
            {
                for (name, entry) in meta.iter_mut() {
                    if name.as_str() == Some("signer") {
                        *entry = Value::Bin(vec![0x55; ADDRESS_HASH_LEN]);
                    }
                }
            }
        }
        let mut forged = signature.to_vec();
        forged.extend_from_slice(&msgpack::encode(&Value::Map(fields)));
        assert_eq!(validate(&forged, None, None), Err(Error::SignerMismatch));
    }

    #[test]
    fn a_truncated_artifact_is_refused_rather_than_indexed() {
        let signer = oracle_identity();
        let artifact = create(&signer, b"payload", true, &[]).unwrap();
        assert_eq!(
            validate(&artifact[..SIGNATURE_LEN], None, None),
            Err(Error::TooShort)
        );
        assert_eq!(validate(&[], None, None), Err(Error::TooShort));
        for cut in [SIGNATURE_LEN + 1, SIGNATURE_LEN + 8, artifact.len() - 1] {
            // Every truncation is an error and none is a panic, which is the whole claim.
            assert!(validate(&artifact[..cut], None, None).is_err());
        }
    }
}
