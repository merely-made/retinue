//! Keyed semantic replay tags kept beside, but never reconstructed from, journal state.

use core::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::super::super::{NodeId, Request, VERSION, VerifiedController};
use super::SEMANTIC_TAG_LEN;

type HmacSha256 = Hmac<Sha256>;

const DOMAIN: &[u8] = b"radio-hand-semantic-tag-v2";

/// Firmware-held HMAC key for durable semantic replay tags.
///
/// Firmware must retain the same high-entropy key across reboot and keep it unavailable to
/// carriers. Its board-specific derivation and sealing are runtime work; this journal only
/// consumes the resulting key.
pub struct SemanticTagKey([u8; 32]);

impl SemanticTagKey {
    /// Wrap key material supplied or derived by board firmware.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for SemanticTagKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SemanticTagKey([redacted])")
    }
}

/// Truncated HMAC-SHA256 over a canonical, authenticated control request.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SemanticTag([u8; SEMANTIC_TAG_LEN]);

impl SemanticTag {
    pub(super) fn derive(
        key: &SemanticTagKey,
        node: NodeId,
        controller: VerifiedController,
        request: &Request,
    ) -> Self {
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(&key.0)
            .expect("HMAC-SHA256 accepts every key length");
        mac.update(DOMAIN);
        mac.update(&node.0);
        mac.update(&(controller.0).0);
        mac.update(&[VERSION]);
        mac.update(&request.transaction.0);
        mac.update(&request.transaction_sequence.to_le_bytes());
        mac.update(&request.expected_generation.0.to_le_bytes());
        mac.update(&[request.operation as u8]);
        let arguments_len = u16::try_from(request.arguments.len())
            .expect("the bounded WN0 request length fits in u16");
        mac.update(&arguments_len.to_le_bytes());
        mac.update(&request.arguments);

        let digest = mac.finalize().into_bytes();
        let mut bytes = [0; SEMANTIC_TAG_LEN];
        bytes.copy_from_slice(&digest[..SEMANTIC_TAG_LEN]);
        Self(bytes)
    }

    pub(super) const fn as_bytes(&self) -> &[u8; SEMANTIC_TAG_LEN] {
        &self.0
    }

    pub(super) const fn from_persisted(bytes: [u8; SEMANTIC_TAG_LEN]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for SemanticTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SemanticTag([redacted])")
    }
}
