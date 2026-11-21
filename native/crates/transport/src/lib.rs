// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! # Mere-Transport
//!
//! Peer transport layer for the [`mere`](https://crates.io/crates/mere)
//! browser workspace. Wraps [iroh](https://www.iroh.computer) for
//! authenticated, encrypted QUIC streams between known peers, and exposes
//! the [`Transport`] trait the rest of the workspace consumes.
//!
//! ## Design
//!
//! - **Identity** is provider-neutral at the transport boundary: Mere callers
//!   can supply an [`identity`] keypair, while external providers can supply a
//!   raw Ed25519 seed. The same signing key determines the [`PeerID`].
//! - **Streams are byte-oriented**: the [`Transport::Stream`] associated
//!   type is `AsyncRead + AsyncWrite`, so consumers can layer their own
//!   framing (Cable's p2panda operations, MLS's TLS-style, etc.) on top.
//! - **ALPNs are explicit**: each protocol registers its own ALPN string
//!   (`mere/cable/v1`, `mere/coop/v1`, etc.) so multiple protocols can share
//!   one peer connection without ambiguity.
//! - **Generic over implementation**: consumers take `T: Transport` rather
//!   than `Box<dyn Transport>`, so the same code works against real iroh in
//!   production and against an in-memory transport in tests.
//!
//! ## Status
//!
//! Pre-1.0. The trait surface and the in-memory test fixture
//! ([`memory::MemoryTransport`]) shipped in 0.0.1 (Phase 2B).
//! [`p2panda_transport::P2pandaTransport`] is the production transport: it makes
//! p2panda-net's `Endpoint` the endpoint authority (gaining discovery + sync +
//! relay/hole-punching), and replaced a hand-rolled iroh `Router`.
//!
//! ## Consumers
//!
//! - [`murm`](https://crates.io/crates/murm) — Cable rides on transport
//!   streams (per inherited Cable spec §2.1)
//! - [`gemot`](https://crates.io/crates/gemot) — community/federation
//!   sync (planned)
//! - Future: [`eidetic`](https://crates.io/crates/eidetic) sync, co-op session orchestration

#![doc(html_root_url = "https://docs.rs/transport/0.0.1")]
#![warn(missing_docs)]

mod accepted;
mod alpn;
pub mod blob_source;
pub mod blobs;
mod error;
pub mod memory;
#[cfg(feature = "i2p")]
pub mod i2p;
/// I2P as a [`Transport`], each stream a Noise session (graphshell:mere-cli D12).
#[cfg(feature = "i2p-transport")]
pub mod i2p_transport;
/// An encrypted session layer that composes *over* a carrier's stream, rather
/// than being a carrier itself. See the module docs for the layering.
#[cfg(feature = "noise")]
pub mod noise;
#[cfg(feature = "notochord")]
pub mod notochord;
mod outbound;
mod p2panda_host;
pub mod p2panda_transport;
mod peer_id;
mod peer_route;
#[cfg(feature = "reticulum")]
pub mod reticulum_transport;
/// Owner-selected session security with bounded setup and admission leases.
#[cfg(feature = "noise")]
pub mod security;
#[cfg(feature = "sync-lane")]
pub mod sync_lane;
mod transport;

pub use crate::accepted::{AcceptedSession, IngressContext, IngressInterfaceId, TransportKind};
pub use crate::alpn::Alpn;
pub use crate::blobs::{
    BlobError, BlobHash, BlobLease, BlobPeerAuthorizer, BlobReadAuthorizer, BlobScope, BlobStore,
};
pub use crate::error::TransportError;
pub use crate::outbound::OutboundSession;
pub use crate::p2panda_host::{P2pandaHostPolicy, P2pandaOverlayHost, RejectedPeerHint};
pub use crate::p2panda_transport::{
    BoundedAcceptHandle, BoundedAcceptStats, P2pandaStream, P2pandaTransport, sync_overlay_topic,
};
// The gossip handle returned by `P2pandaTransport::subscribe` (space live-sync):
// `publish(bytes)` to broadcast, `subscribe()` for the received-bytes stream.
#[cfg(feature = "notochord")]
pub use crate::notochord::{initiator_binding, initiator_link_binding};
pub use crate::peer_id::PeerID;
pub use crate::peer_route::{PeerAddr, PeerPath, decode_peer_ticket, encode_peer_ticket};
#[cfg(feature = "reticulum")]
pub use crate::reticulum_transport::{
    ReticulumInterface, ReticulumStream, ReticulumTransport, ReticulumTransportBuilder,
};
pub use crate::transport::Transport;
pub use p2panda_net::gossip::GossipHandle;

// Re-export commonly-used identity types so consumers don't need a direct
// dependency on `identity` for the basic identity-into-transport flow.
pub use identity::{Ed25519PublicKey, IdentityProvider};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Lifecycle stage marker.
pub const STAGE: &str = "pre-alpha";

#[cfg(test)]
mod tests {
    use super::*;
    use identity::InMemoryProvider;

    #[test]
    fn alpn_from_str_round_trips() {
        let alpn = Alpn::new("mere/cable/v1");
        assert_eq!(alpn.as_bytes(), b"mere/cable/v1");
        assert_eq!(alpn.len(), 13);
        assert!(!alpn.is_empty());
    }

    #[test]
    fn empty_alpn_reports_empty() {
        let alpn = Alpn::new("");
        assert!(alpn.is_empty());
        assert_eq!(alpn.len(), 0);
    }

    #[test]
    fn alpn_equality_and_hashing() {
        use std::collections::HashSet;
        let a = Alpn::new("mere/cable/v1");
        let b = Alpn::new("mere/cable/v1");
        let c = Alpn::new("mere/coop/v1");
        assert_eq!(a, b);
        assert_ne!(a, c);

        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }

    #[test]
    fn peer_id_round_trips_through_bytes() {
        let provider = InMemoryProvider::from_seed([42; 32]);
        let pk = provider.master_public_key();
        let peer_id = PeerID::from_public_key(pk);
        let bytes = peer_id.to_bytes();
        let recovered = PeerID::from_bytes(&bytes).unwrap();
        assert_eq!(peer_id, recovered);
    }

    #[test]
    fn peer_id_carries_public_key_equality() {
        let p1 = InMemoryProvider::from_seed([7; 32]);
        let p2 = InMemoryProvider::from_seed([7; 32]);
        let p3 = InMemoryProvider::from_seed([8; 32]);

        let n1 = PeerID::from_public_key(p1.master_public_key());
        let n2 = PeerID::from_public_key(p2.master_public_key());
        let n3 = PeerID::from_public_key(p3.master_public_key());

        assert_eq!(n1, n2);
        assert_ne!(n1, n3);
    }

    #[test]
    fn peer_id_works_as_hashmap_key() {
        use std::collections::HashMap;
        let p1 = InMemoryProvider::from_seed([1; 32]);
        let p2 = InMemoryProvider::from_seed([2; 32]);
        let n1 = PeerID::from_public_key(p1.master_public_key());
        let n2 = PeerID::from_public_key(p2.master_public_key());

        let mut map = HashMap::new();
        map.insert(n1, "peer-one");
        map.insert(n2, "peer-two");

        assert_eq!(map.get(&n1), Some(&"peer-one"));
        assert_eq!(map.get(&n2), Some(&"peer-two"));
    }

    #[test]
    fn peer_id_from_implements_from_pubkey() {
        let p = InMemoryProvider::from_seed([13; 32]);
        let pk = p.master_public_key();
        let peer_id_via_from: PeerID = pk.into();
        let peer_id_via_method = PeerID::from_public_key(pk);
        assert_eq!(peer_id_via_from, peer_id_via_method);
    }
}
