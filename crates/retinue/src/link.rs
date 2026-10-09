//! Links: the handshake, the established link, and link-data proofs.
//!
//! An RNS 1.x link request carries 67 bytes (two ephemeral keys and a 3-byte trailer); a
//! link proof carries 99 (signature, public key, trailer). The trailer is 24-bit big-endian:
//!
//! ```text
//! bits 23..21  the AES mode   (0 = AES-128-CBC, 1 = AES-256-CBC)
//! bits 20..0   the MTU
//! ```
//!
//! Observed: an initiator sends `20 20 00` (mode 1, MTU 8192); the responder answers
//! `20 01 f4` (mode 1, MTU 500 = `Reticulum.MTU`). Beechat sends a bare 64-byte request.
//!
//! ```text
//! link_id = trunc16(SHA256( (flags & 0x0F) || destination(16) || context(1) || payload[..64] ))
//! ```
//!
//! `hops` is excluded because it mutates in transit, and the negotiable trailer is excluded
//! by truncating to the keys. Solved against two captured (request, link id) pairs; see
//! `oracle/capture_link.py`.

mod established;
mod handshake;
mod proof;
mod wire;

#[cfg(test)]
mod tests;

pub use established::{Inbound, Link};
pub use handshake::{PendingLink, accept};
pub use proof::{DATA_PROOF_LEN, LINK_IDENTIFY_LEN, data_proof_packet, read_data_proof};
pub use wire::{
    CTX_CACHE_REQUEST, CTX_CHANNEL, CTX_KEEPALIVE, CTX_LINKCLOSE, CTX_LINKIDENTIFY, CTX_LRPROOF,
    CTX_LRRTT, CTX_REQUEST, CTX_RESOURCE, CTX_RESOURCE_ADV, CTX_RESOURCE_HMU, CTX_RESOURCE_ICL,
    CTX_RESOURCE_PRF, CTX_RESOURCE_RCL, CTX_RESOURCE_REQ, CTX_RESPONSE, KEEPALIVE_REQUEST,
    KEEPALIVE_RESPONSE, LINK_KEYS_LEN, LINK_PROOF_LEN, LINK_REQUEST_LEN, LinkMode, LinkTrailer,
    MAX_MTU, TRAILER_LEN, clamp_request_mtu, link_id, proof_is_signed_by, request_trailer,
};
