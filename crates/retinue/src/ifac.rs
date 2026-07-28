//! Per-interface authentication and virtual-network separation.
//!
//! IFAC is a carrier envelope, not part of a logical Reticulum packet. An ingress
//! interface unmasks and verifies it before [`crate::packet::Packet::decode`]; an
//! egress interface signs and masks the logical packet again with its own
//! credentials.

use core::fmt;

use ed25519_dalek::{Signer, SigningKey};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// Smallest supported access code.
pub const MIN_SIZE: usize = 1;
/// Largest supported access code: one complete Ed25519 signature.
pub const MAX_SIZE: usize = 64;
/// Reticulum's usual access-code size for stream interfaces.
pub const DEFAULT_SIZE: usize = 8;

const IFAC_SALT: [u8; 32] = [
    0xad, 0xf5, 0x4d, 0x88, 0x2c, 0x9a, 0x9b, 0x80, 0x77, 0x1e, 0xb4, 0x99, 0x5d, 0x70, 0x2d, 0x4a,
    0x3e, 0x73, 0x33, 0x91, 0xb2, 0xa0, 0xf5, 0x3f, 0x41, 0x6d, 0x9f, 0x90, 0x7e, 0x55, 0xcf, 0xf8,
];

/// Invalid IFAC configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Neither a network name nor a passphrase was supplied.
    MissingCredentials,
    /// The access code was outside the protocol's 1–64-byte range.
    InvalidSize,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCredentials => {
                f.write_str("IFAC needs a network name, a passphrase, or both")
            }
            Self::InvalidSize => f.write_str("IFAC size must be between 1 and 64 bytes"),
        }
    }
}

impl core::error::Error for ConfigError {}

/// Shared credentials and code length for one authenticated interface.
#[derive(Clone)]
pub struct Ifac {
    key: [u8; 64],
    signing: SigningKey,
    size: usize,
}

impl fmt::Debug for Ifac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ifac").field("size", &self.size).finish()
    }
}

impl Ifac {
    /// Derive an interface identity from a network name and/or passphrase.
    ///
    /// `size` is measured in bytes. Reticulum configuration files express the
    /// same value in bits.
    pub fn new(
        network_name: Option<&str>,
        passphrase: Option<&str>,
        size: usize,
    ) -> core::result::Result<Self, ConfigError> {
        if network_name.is_none() && passphrase.is_none() {
            return Err(ConfigError::MissingCredentials);
        }
        if !(MIN_SIZE..=MAX_SIZE).contains(&size) {
            return Err(ConfigError::InvalidSize);
        }

        let mut origin = Vec::with_capacity(64);
        if let Some(name) = network_name {
            origin.extend_from_slice(&Sha256::digest(name.as_bytes()));
        }
        if let Some(phrase) = passphrase {
            origin.extend_from_slice(&Sha256::digest(phrase.as_bytes()));
        }
        let origin_hash = Sha256::digest(&origin);

        let hkdf = Hkdf::<Sha256>::new(Some(&IFAC_SALT), &origin_hash);
        let mut key = [0_u8; 64];
        hkdf.expand(&[], &mut key)
            .expect("64 bytes is a valid SHA-256 HKDF output");

        let seed: [u8; 32] = key[32..]
            .try_into()
            .expect("the IFAC identity key contains an Ed25519 seed");
        Ok(Self {
            key,
            signing: SigningKey::from_bytes(&seed),
            size,
        })
    }

    /// Derive credentials using the usual eight-byte access code.
    pub fn with_default_size(
        network_name: Option<&str>,
        passphrase: Option<&str>,
    ) -> core::result::Result<Self, ConfigError> {
        Self::new(network_name, passphrase, DEFAULT_SIZE)
    }

    /// Number of bytes this envelope adds to every packet.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Sign and mask one logical packet for this interface.
    pub fn seal(&self, packet: &[u8]) -> Result<Vec<u8>> {
        if packet.len() < 2 {
            return Err(Error::Truncated);
        }
        if packet.len() > crate::packet::MTU {
            return Err(Error::Oversize);
        }

        let mut logical = packet.to_vec();
        logical[0] &= 0x7f;
        let ifac = self.code(&logical);

        let mut wire = Vec::with_capacity(logical.len() + self.size);
        wire.push(logical[0] | 0x80);
        wire.push(logical[1]);
        wire.extend_from_slice(&ifac);
        wire.extend_from_slice(&logical[2..]);

        self.apply_mask(&mut wire, &ifac);
        Ok(wire)
    }

    /// Unmask and authenticate one interface frame.
    ///
    /// Successful output is the original logical packet with the IFAC flag
    /// cleared and the access-code field removed.
    pub fn open(&self, frame: &[u8]) -> Result<Vec<u8>> {
        if frame.len() < 2 + self.size {
            return Err(Error::Truncated);
        }
        if frame.len() > crate::packet::MTU + self.size {
            return Err(Error::Oversize);
        }

        let ifac = &frame[2..2 + self.size];
        let mut unmasked = frame.to_vec();
        self.apply_mask(&mut unmasked, ifac);
        if unmasked[0] & 0x80 == 0 {
            return Err(Error::BadIfac);
        }

        let mut logical = Vec::with_capacity(frame.len() - self.size);
        logical.push(unmasked[0] & 0x7f);
        logical.push(unmasked[1]);
        logical.extend_from_slice(&unmasked[2 + self.size..]);

        let expected = self.code(&logical);
        let different = expected
            .iter()
            .zip(ifac)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            });
        if different != 0 {
            return Err(Error::BadIfac);
        }
        Ok(logical)
    }

    fn code(&self, packet: &[u8]) -> Vec<u8> {
        let signature = self.signing.sign(packet).to_bytes();
        signature[signature.len() - self.size..].to_vec()
    }

    fn apply_mask(&self, packet: &mut [u8], ifac: &[u8]) {
        let hkdf = Hkdf::<Sha256>::new(Some(&self.key), ifac);
        let mut mask = vec![0_u8; packet.len()];
        hkdf.expand(&[], &mut mask)
            .expect("a Reticulum packet is a valid SHA-256 HKDF output");
        for (index, (byte, mask_byte)) in packet.iter_mut().zip(mask).enumerate() {
            if index <= 1 || index >= 2 + self.size {
                *byte ^= mask_byte;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGICAL_HEX: &str =
        "0800c3421c254bb3b6e25d8040f395d960f900726574696e75652d696661632d66697874757265";
    const ORACLE_WIRE: &[u8] = include_bytes!("../tests/fixtures/ifac_packet.bin");

    fn oracle_ifac() -> Ifac {
        Ifac::new(Some("retinue-ifac"), Some("wire-compatibility"), 8).unwrap()
    }

    #[test]
    fn matches_pinned_rns_1_4_0_wire_bytes() {
        let logical = hex::decode(LOGICAL_HEX).unwrap();
        let ifac = oracle_ifac();
        assert_eq!(ifac.seal(&logical).unwrap(), ORACLE_WIRE);
        assert_eq!(ifac.open(ORACLE_WIRE).unwrap(), logical);
    }

    #[test]
    fn wrong_credentials_and_mutation_are_rejected() {
        let wrong = Ifac::new(Some("retinue-ifac"), Some("wrong"), 8).unwrap();
        assert_eq!(wrong.open(ORACLE_WIRE), Err(Error::BadIfac));

        let mut changed = ORACLE_WIRE.to_vec();
        *changed.last_mut().unwrap() ^= 1;
        assert_eq!(oracle_ifac().open(&changed), Err(Error::BadIfac));
    }

    #[test]
    fn every_protocol_code_size_round_trips() {
        let logical = hex::decode(LOGICAL_HEX).unwrap();
        for size in MIN_SIZE..=MAX_SIZE {
            let ifac = Ifac::new(Some("private-ret"), None, size).unwrap();
            let wire = ifac.seal(&logical).unwrap();
            assert_eq!(wire.len(), logical.len() + size);
            assert_eq!(ifac.open(&wire).unwrap(), logical);
        }
    }

    #[test]
    fn credentials_and_size_are_required() {
        assert!(matches!(
            Ifac::new(None, None, DEFAULT_SIZE),
            Err(ConfigError::MissingCredentials)
        ));
        assert!(matches!(
            Ifac::new(Some("name"), None, 0),
            Err(ConfigError::InvalidSize)
        ));
        assert!(matches!(
            Ifac::new(Some("name"), None, MAX_SIZE + 1),
            Err(ConfigError::InvalidSize)
        ));
    }

    #[test]
    fn oversized_logical_and_carrier_packets_are_rejected() {
        let ifac = oracle_ifac();
        assert_eq!(
            ifac.seal(&vec![0_u8; crate::packet::MTU + 1]),
            Err(Error::Oversize)
        );
        assert_eq!(
            ifac.open(&vec![0_u8; crate::packet::MTU + ifac.size() + 1]),
            Err(Error::Oversize)
        );
    }
}
