//! Fixed-size public-identity syntax validation shared by durable control code.
//!
//! A Retinue public identity is `x25519_public(32) || ed25519_verifying(32)`.
//! X25519 public bytes are intentionally unconstrained here; only the Ed25519
//! verifying-key encoding has syntax to validate.

use ed25519_dalek::VerifyingKey;

/// Exact byte length of one Retinue public identity.
pub const RETINUE_PUBLIC_IDENTITY_LEN: usize = 64;
const ED25519_OFFSET: usize = 32;

/// Why a fixed-size Retinue public identity is syntactically invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicIdentityError {
    /// The slice is not exactly one public identity.
    Length,
    /// The Ed25519 verifying-key half is not a valid encoded point.
    InvalidEd25519,
}

/// Validates one Retinue public identity without allocation or Retinue runtime types.
pub fn validate_retinue_public_identity(bytes: &[u8]) -> Result<(), PublicIdentityError> {
    let bytes: &[u8; RETINUE_PUBLIC_IDENTITY_LEN] =
        bytes.try_into().map_err(|_| PublicIdentityError::Length)?;
    let ed25519 = bytes[ED25519_OFFSET..]
        .try_into()
        .expect("the second half of a 64-byte identity is 32 bytes");
    VerifyingKey::from_bytes(ed25519).map_err(|_| PublicIdentityError::InvalidEd25519)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn valid_identity(seed: u8) -> [u8; RETINUE_PUBLIC_IDENTITY_LEN] {
        let mut identity = [seed; RETINUE_PUBLIC_IDENTITY_LEN];
        identity[ED25519_OFFSET..].copy_from_slice(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .as_bytes(),
        );
        identity
    }

    #[test]
    fn accepts_arbitrary_x25519_bytes_and_a_valid_ed25519_half() {
        let identity = valid_identity(0x51);
        assert_eq!(validate_retinue_public_identity(&identity), Ok(()));
    }

    #[test]
    fn rejects_truncation_and_malformed_ed25519_without_touching_x25519_rules() {
        assert_eq!(
            validate_retinue_public_identity(&valid_identity(0x52)[..63]),
            Err(PublicIdentityError::Length)
        );
        let mut malformed = [0; RETINUE_PUBLIC_IDENTITY_LEN];
        // This compressed Edwards-Y encoding does not decompress as an Ed25519 key.
        malformed[ED25519_OFFSET..].fill(2);
        assert_eq!(
            validate_retinue_public_identity(&malformed),
            Err(PublicIdentityError::InvalidEd25519)
        );
    }
}
