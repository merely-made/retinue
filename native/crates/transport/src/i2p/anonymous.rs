// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! The SAM destination of an anonymous carrier (release log D3 and R7).
//!
//! The destination's keys come from an [`UnassociatedKeypair`], never from a
//! persona or a derived key: the argument type admits nothing else. They are
//! HKDF children of the key's seed under their own labels, so the destination
//! shares no public bytes with the key's p2panda or Reticulum identity, and
//! stays stable across restarts without being stored apart from the key.
//!
//! Layout is I2P's destination with a key certificate for EdDSA-SHA512-Ed25519
//! signing (type 7) and X25519 encryption (type 4), the pair `TunnelProfile`
//! asks the router to publish (`i2cp.leaseSetEncType` 4). The padding in the
//! two public key fields is derived from the seed too, so it carries nothing
//! from any other key.

use hkdf::Hkdf;
use identity::{Ed25519Keypair, UnassociatedKeypair};
use sha2::Sha256;
use truchman::{PrivateKey, base64_encode};
use x25519_dalek::{X25519_BASEPOINT_BYTES, x25519};
use zeroize::Zeroize;

const SIGNING_INFO: &[u8] = b"mere/i2p-destination/v1/ed25519";
const ENCRYPTION_INFO: &[u8] = b"mere/i2p-destination/v1/x25519";
const PADDING_INFO: &[u8] = b"mere/i2p-destination/v1/padding";
/// Key certificate: type 5, length 4, signing type 7, crypto type 4.
const CERTIFICATE: [u8; 7] = [5, 0, 4, 0, 7, 0, 4];
const ENCRYPTION_FIELD: usize = 256;
const SIGNING_FIELD: usize = 128;

/// The SAM private key an anonymous carrier's session speaks for.
///
/// # Panics
/// Never in practice: the assembled bytes always satisfy the destination format.
pub fn anonymous_i2p_key(key: &UnassociatedKeypair) -> PrivateKey {
    let mut seed = key.to_seed();
    let hk = Hkdf::<Sha256>::new(None, &seed);
    seed.zeroize();
    let expand = |info: &[u8], out: &mut [u8]| {
        hk.expand(info, out)
            .expect("HKDF-SHA256 output length is valid");
    };

    let mut signing_seed = [0u8; 32];
    let mut encryption_secret = [0u8; 32];
    let mut padding = [0u8; ENCRYPTION_FIELD - 32 + SIGNING_FIELD - 32];
    expand(SIGNING_INFO, &mut signing_seed);
    expand(ENCRYPTION_INFO, &mut encryption_secret);
    expand(PADDING_INFO, &mut padding);

    let signing_public = Ed25519Keypair::from_seed(signing_seed)
        .public_key()
        .to_bytes();
    let encryption_public = x25519(encryption_secret, X25519_BASEPOINT_BYTES);
    let (encryption_pad, signing_pad) = padding.split_at(ENCRYPTION_FIELD - 32);

    let mut bytes = Vec::with_capacity(ENCRYPTION_FIELD + SIGNING_FIELD + CERTIFICATE.len() + 64);
    bytes.extend_from_slice(&encryption_public);
    bytes.extend_from_slice(encryption_pad);
    bytes.extend_from_slice(signing_pad);
    bytes.extend_from_slice(&signing_public);
    bytes.extend_from_slice(&CERTIFICATE);
    bytes.extend_from_slice(&encryption_secret);
    bytes.extend_from_slice(&signing_seed);

    let mut encoded = base64_encode(&bytes);
    bytes.zeroize();
    signing_seed.zeroize();
    encryption_secret.zeroize();
    let parsed = PrivateKey::from_base64(&encoded).expect("a well-formed private key");
    encoded.zeroize();
    parsed
}

#[cfg(test)]
mod tests {
    use identity::{IdentityProvider, InMemoryProvider};
    use truchman::base64_decode;

    use super::*;

    fn bytes(key: &PrivateKey) -> Vec<u8> {
        base64_decode(key.expose_base64()).expect("base64")
    }

    #[test]
    fn destination_is_stable_and_independent_of_every_persona_key() {
        let persona = InMemoryProvider::from_seed([71; 32]);
        let key = UnassociatedKeypair::generate();
        let first = anonymous_i2p_key(&key);
        assert_eq!(first, anonymous_i2p_key(&key), "stable across restarts");
        assert_ne!(first, anonymous_i2p_key(&UnassociatedKeypair::generate()));

        let raw = bytes(&first);
        let public = first.destination().as_bytes();
        assert_eq!(public.len(), 391);
        assert_eq!(&public[384..], &CERTIFICATE);
        let signing = &raw[ENCRYPTION_FIELD + SIGNING_FIELD - 32..ENCRYPTION_FIELD + SIGNING_FIELD];
        let signing_seed: [u8; 32] = raw[raw.len() - 32..].try_into().unwrap();
        assert_eq!(
            signing,
            Ed25519Keypair::from_seed(signing_seed)
                .public_key()
                .to_bytes()
        );

        // Neither the carrier key, nor any persona key, appears anywhere in
        // the destination or its private half.
        let carrier = key.public_key().to_bytes();
        let derived = persona.derived_keypair(b"anon").unwrap();
        for known in [
            carrier,
            persona.master_public_key().to_bytes(),
            derived.public_key().to_bytes(),
        ] {
            assert!(!raw.windows(32).any(|w| w == known), "no shared key bytes");
        }
        assert!(
            !raw.windows(32).any(|w| w == key.to_seed()),
            "the seed is not the destination's"
        );
        assert_ne!(
            first.destination().b32(),
            anonymous_i2p_key(&UnassociatedKeypair::generate())
                .destination()
                .b32()
        );
    }
}
