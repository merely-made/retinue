//! A sans-io MeshCore node: the pipeline that ties identity, cipher, message, dedup, and
//! forwarding together.
//!
//! It holds no radio and no clock. A caller (a pump over a [`tulle`](https://github.com/mark-ik/tulle)
//! modem, or an in-process test) feeds received frames to [`Node::on_frame`] and transmits the
//! frames the node emits — retransmissions for flood/direct routing, plus whatever the app
//! composes with [`Node::advert_frame`], [`Node::text_frame`], and [`Node::ack_frame`].
//!
//! Receive pipeline: decode the packet, drop flood duplicates ([`crate::mesh::SeenTable`]),
//! dispatch by payload type (verify and learn adverts; decrypt text addressed to us and
//! surface its ack; note acks), then decide retransmission ([`crate::mesh::route_recv`]).
//!
//! Text messaging works only after the two ends have heard each other's adverts, since the
//! per-pair cipher key is ECDH over the peer's public key. A first message floods when no route
//! is known; its authenticated PATH response establishes reciprocal direct routes for later
//! messages. V1 payload identity hashes remain one byte; routes support one-,
//! two- and three-byte public-key prefixes.
//!
//! Ported from upstream MeshCore (MIT, <https://github.com/ripplebiz/MeshCore>).

mod capacity;
mod pending;
mod receive;
mod route;
mod send;
mod state;
#[cfg(test)]
mod tests;

pub use capacity::{CapacityError, NodeCapacity};
pub use pending::{PendingText, PendingTexts, TextAttempt, TextRetryPolicy};
pub use route::DirectRoute;
pub use state::{Event, Node};

/// The largest UTF-8 text body that fits the current one-frame private text
/// format. It is a wire limit, independent of any node capacity.
pub use crate::message::MAX_TEXT_BYTES;
