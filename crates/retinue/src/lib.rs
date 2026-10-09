#![forbid(unsafe_code)]

//! retinue — an endpoint-scoped implementation of the
//! [Reticulum](https://reticulum.network/) protocol.
//!
//! A retinue is the company that travels with a person. This crate is that for a peer: the
//! identity, announce, link, resource, and reliable-stream layers a node needs to *be* a
//! Reticulum endpoint, embedded as a library, qualified against RNS 1.5.4 within the
//! measured local interoperability scope.
//!
//! # Status
//!
//! The wire vocabulary, links, resources, request/response, the endpoint runtime, opt-in
//! transport-node routing, and reliable streaming are implemented. RNS 1.5.4 live
//! gates check local interoperability; byte fixtures retain their actual producer
//! versions, including the historical 1.3.8 corpus. These receipts do not establish
//! full upstream parity or requalify firmware. See *Provenance*. The
//! layering:
//!
//! - **Allocation-free floor** — always available, no heap: identities ([`identity`]),
//!   hashes ([`hash`]), the signed command envelope ([`command`]), and the table-size
//!   constants ([`capacity`]). This is all a core-only firmware image needs to verify an
//!   operator's command, so it is the only part that builds with the `alloc` feature off.
//! - **Sans-io core** — behind the `alloc` feature (on by default), no runtime or RNG: the
//!   packet codec (`packet`), announces (`announce`), the token (`token`), HDLC framing
//!   (`iface::hdlc`), links (`link`), resources (`resource`), and the `Channel`/`Buffer` +
//!   link-proof reliability machinery (`channel`, `reliable`). Pure functions over bytes,
//!   replayable against fixtures.
//! - **The tokio shell** — behind the `tokio` feature (on by default): the TCP interface
//!   ([`iface::tcp`]) and the [`endpoint`] runtime that attaches interfaces, routes packets,
//!   and opens/accepts links as streams. Turn the feature off and the sans-io core stands
//!   alone.
//!
//! Transport-node routing is opt-in ([`endpoint::Endpoint::enable_routing`], or a full
//! [`endpoint::RoutingPolicy`]); the default posture is endpoint-scoped. On-air interfaces
//! (RNode serial, direct PHY) live in the sibling `tulle` crate and are proven over real
//! RF; endpoint-level resource sessions, route expiry, and announce budgeting are
//! implemented. Ratcheted single packets rotate at announce and retain epochs by count, with
//! a host hook persisting the signed ratchet snapshot before a new ratchet is advertised.
//! Host IFAC virtual-network authentication is applied at TCP and Tulle carrier
//! boundaries. The firmware Node carrier has a separate, still-open IFAC gate.
//! See the README's *Maturity* section and
//! `design_docs/`.
//!
//! # Provenance
//!
//! The current live-oracle target is RNS 1.5.4. This crate was implemented from the
//! public-domain Reticulum protocol specification and the MIT-licensed Beechat
//! `reticulum` crate, with later inputs recorded below and in the notices. Historical
//! oracle work ran and observed the reference implementation. Limited comparative
//! source review began September 26; through October 4 the owner reported no RNS
//! implementation code copied or translated. From October 5 the crate is under the
//! Reticulum License and RNS source may be adapted into it, with each adaptation
//! listed in `NOTICE`. Captured bytes live under `tests/fixtures/`. See
//! `design_docs/2026-07-13_rns_wire_format_reference.md`.
//!
//! The signed-artifact envelope in [`artifact`] had its layout
//! read from [Prns](https://github.com/KenAKAFrosty/Prns) (MIT OR Apache-2.0, MIT elected),
//! which is now withdrawn as a donor and trusted independent reference. Historical
//! donor inputs remain attributed; current signed-artifact captures use project inputs
//! with `rnid` 1.5.2 and 1.5.4. Compatibility does not resolve provenance. See `NOTICE`
//! and `design_docs/2026-08-10_prns_donor_ledger.md`.

#![no_std]

// The sans-io core is `no_std + alloc`: payloads are heap-allocated, but nothing here needs
// an operating system. The floor below it (`command`, `identity`, `hash`, `capacity`) does
// not allocate at all, and a core-only image links just that, so `alloc` is a feature rather
// than a fact. The optional bz2 I/O adapter uses `std` independently of the tokio shell;
// the allocation-only core needs neither. The test harness also imports `std`.
#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "tokio", feature = "compression", test))]
extern crate std;

#[cfg(feature = "alloc")]
pub mod address_book;
#[cfg(feature = "alloc")]
pub mod announce;
#[cfg(feature = "tokio")]
pub mod announce_admission;
#[cfg(feature = "alloc")]
pub mod announce_freshness;
#[cfg(feature = "alloc")]
pub mod artifact;
pub mod capacity;
#[cfg(feature = "alloc")]
pub mod channel;
pub mod command;
#[cfg(feature = "alloc")]
pub mod destination;
#[cfg(feature = "tokio")]
pub mod endpoint;
pub mod hash;
pub mod identity;
#[cfg(feature = "alloc")]
pub mod ifac;
pub mod iface;
#[cfg(feature = "alloc")]
pub mod instance;
#[cfg(feature = "alloc")]
pub mod link;
#[cfg(feature = "alloc")]
pub mod link_liveness;
#[cfg(feature = "alloc")]
pub mod lossy;
#[cfg(feature = "alloc")]
pub mod msgpack;
#[cfg(feature = "alloc")]
pub mod node;
#[cfg(feature = "tokio")]
pub mod nomadnet;
#[cfg(feature = "alloc")]
pub mod packet;
#[cfg(feature = "alloc")]
pub mod path;
#[cfg(all(test, feature = "alloc"))]
mod probe;
#[cfg(feature = "alloc")]
pub mod proof;
#[cfg(feature = "alloc")]
pub mod ratchet;
#[cfg(feature = "alloc")]
pub mod reliable;
#[cfg(feature = "alloc")]
pub mod request;
#[cfg(feature = "alloc")]
pub mod resource;
#[cfg(feature = "alloc")]
pub mod resource_transfer;
#[cfg(feature = "alloc")]
pub mod token;

#[cfg(feature = "alloc")]
pub use address_book::{AddressBook, Peer};
#[cfg(feature = "alloc")]
pub use announce::Announce;
#[cfg(feature = "alloc")]
pub use destination::DestinationName;
pub use hash::{AddressHash, NameHash};
pub use identity::{Identity, PrivateIdentity};
#[cfg(feature = "alloc")]
pub use ifac::Ifac;
#[cfg(feature = "alloc")]
pub use packet::Packet;
#[cfg(feature = "alloc")]
pub use ratchet::{RatchetPolicy, RatchetStore};
#[cfg(feature = "alloc")]
pub use reliable::ReliableChannel;

/// Anything that can go wrong decoding or validating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The input ended before a required field did.
    Truncated,
    /// A packet is larger than the wire MTU. RNS drops such packets; we reject them at the
    /// decoder so a peer cannot hand us an over-sized buffer.
    Oversize,
    /// A public key is not a valid point on its curve.
    BadKey,
    /// The Ed25519 signature did not verify. For an announce this means the peer does not
    /// hold the private key for the identity it is announcing.
    BadSignature,
    /// An interface frame did not carry the expected access code.
    BadIfac,
    /// The destination hash in the header is not the one the announced identity and name
    /// imply: a correctly signed announce for a destination that is not the one claimed.
    DestinationMismatch,
    /// The HMAC on a token did not verify.
    BadMac,
    /// PKCS7 padding was malformed after decryption.
    BadPadding,
    /// An announce decoder was handed a packet that is not an announce.
    NotAnAnnounce,
    /// A link trailer named a cipher mode we do not know.
    BadLinkMode,
    /// A proof decoder was handed a packet that is not a proof.
    NotAProof,
    /// An accept was handed a packet that is not a link request.
    NotALinkRequest,
    /// A proof was addressed to a different link than the one it is being matched against.
    LinkMismatch,
    /// A request or response could not be parsed from its msgpack.
    BadRequest,
    /// A reassembled resource did not match its advertised hash.
    ResourceCorrupt,
    /// The operation needs a feature that is not enabled (e.g. `compression`).
    Unsupported,
    /// A peer asked for more state than this node's capacity policy allows: an
    /// advertisement claiming more parts than the receive limit, or a table already at its
    /// bound. The node stays live and refuses the work.
    CapacityExceeded,
    /// A compressed resource decompressed past the receiver's size limit. The transfer is
    /// failed rather than inflated, so a peer cannot spend this node's memory with a small
    /// bz2 bomb.
    DecompressionLimit,
    /// A resource advertisement named more than one segment. Segment accumulation is not
    /// implemented, so the offer is refused rather than truncated to its first segment.
    MultiSegmentResource,
    /// A resource offer was refused by this side's accept policy, before any part of it
    /// was requested. The sender was told with a receiver cancel.
    ResourceRejected,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Self::Truncated => "input ended mid-field",
            Self::Oversize => "packet exceeds the wire MTU",
            Self::BadKey => "invalid public key",
            Self::BadSignature => "signature did not verify",
            Self::BadIfac => "interface access code did not verify",
            Self::DestinationMismatch => "destination hash does not match the announced identity",
            Self::BadMac => "token HMAC did not verify",
            Self::BadPadding => "malformed padding",
            Self::NotAnAnnounce => "packet is not an announce",
            Self::BadLinkMode => "unknown link cipher mode",
            Self::NotAProof => "packet is not a proof",
            Self::NotALinkRequest => "packet is not a link request",
            Self::LinkMismatch => "proof is for a different link",
            Self::BadRequest => "malformed request or response",
            Self::ResourceCorrupt => "reassembled resource does not match its hash",
            Self::Unsupported => "operation needs a disabled feature",
            Self::CapacityExceeded => "peer asked for more state than the capacity policy allows",
            Self::DecompressionLimit => "decompressed resource exceeds the size limit",
            Self::MultiSegmentResource => "multi-segment resources are not supported",
            Self::ResourceRejected => "resource offer refused by the accept policy",
        };
        f.write_str(s)
    }
}

impl core::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
