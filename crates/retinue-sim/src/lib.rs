#![forbid(unsafe_code)]
//! Run Retinue nodes in process over a shared-radio topology, and record what happened.
//!
//! Each radio is a real [`retinue::node::Node`]. The harness is its shell: it polls every
//! node on a simulated millisecond clock, carries each transmitted frame to every neighbour
//! over an uncut edge, and feeds the bytes back through `Packet::decode` and `Node::ingest`.
//! Nothing is scripted about routing; the trace is whatever the protocol did.
//!
//! # Determinism
//!
//! The same [`Scenario`] yields the same trace, byte for byte. Identities, announce
//! nonces, link seeds and IVs are derived from node names and per-node counters. The event
//! queue orders by time, then by scheduling order: scenario cuts and sends first, then each
//! node's polls in topology order, then frame arrivals as they are transmitted. Neighbours
//! hear a frame in topology order.
//!
//! # The medium
//!
//! One interface per node. A transmission is heard by every neighbour whose edge is not cut
//! at the moment it starts, `hop_delay` later. There is no collision, airtime, loss or
//! signal model, so the trace carries no RSSI or SNR. A cut silences an edge in both
//! directions for the rest of the run.
//!
//! # Applications
//!
//! A [`Send`] opens a link from its sender to its destination with `Node::open_link`, which
//! addresses the request to the route's first relay. When the link comes up at the sender,
//! the payload goes out with `Node::send`; it is delivered when the destination's node
//! returns it as `Action::Data`. A send whose link never comes up is undelivered and is not
//! retried. Its sender's `Node` drops the request at its deadline
//! (`retinue::node::link_request_timeout`), at the first poll or `open_link` at or after
//! it, freeing the pending slot. That is a `link_request_expired` event: `pending_links`
//! falls and the face reads "link unanswered", a `failed` event, where an established link
//! ending reads "link down".
//!
//! # The trace, schema `retinue-sim.route-trace/v1`
//!
//! [`Trace::to_json`] is the canonical form: compact JSON, fields in declaration order,
//! hashes in lowercase hex, times in simulated milliseconds. The top level is
//!
//! - `schema`: [`SCHEMA`];
//! - `scenario`, `profile` (the `Node` table bounds), `timing`, `nodes` (name, destination
//!   and identity hash, transit), `edges`, `cuts` and `sends`, echoing the input;
//! - `events`: every step, in order, each tagged by `kind`;
//! - `messages`: each send's link and, if it arrived, its delivery time and path.
//!
//! Event kinds:
//!
//! - `cut`: an edge went silent.
//! - `send`: an application send began, with the relay its request is addressed to.
//! - `send_refused`: the sender's node would not open a link.
//! - `transmit`: a node put a frame on the air. `origin` is `announce`, `app`, `reply` (an
//!   answer to a received frame, such as a proof) or `forward` (the received frame itself,
//!   relayed). `cause` names the received frame for the last two. `heard_by` and `blocked`
//!   split the sender's neighbours by the cuts.
//! - `receive`: a node heard a frame; `effects` are the non-send actions its node returned.
//! - `link_request_expired`: a link request the node opened got no proof by its deadline
//!   and was dropped, with its `link` id and the `message` it was opened for. It comes from
//!   a poll, or from `open_link` ahead of the `send` whose slot it freed. The medium has no
//!   airtime, so the deadline carries no first-hop allowance.
//! - `delivered`: a payload reached its destination, with the forwarding path of its data
//!   frame, sender first.
//!
//! `transmit`, `receive` and `link_request_expired` carry the acting node's [`NodeState`]
//! after the event. A consumer drawing a node's face at step *i* uses that node's latest
//! state at or before *i*. Its fields map onto radio-face's TRAFFIC page and ticker as the
//! Retinue channel node fills them; [`NodeState`] gives the mapping.
//!
//! A change that would break a reader of v1 takes a new schema id. `link_request_expired`
//! joined v1 on 2026-10-02 (Ruling 55), before any consumer of the schema existed.
//!
//! # Faces
//!
//! With the `face` feature, `face::face` maps a [`NodeState`] onto radio-face's
//! `LocalStatus` and `HostSnapshot`, and `face::FaceTrack` derives a separate file from a
//! trace, schema `retinue-sim.face-track/v1`: each state-carrying event's face as the JSON
//! documents radio-mirror reads. The route trace is unchanged by it.

#[cfg(feature = "face")]
pub mod face;
pub mod scenario;
pub mod sim;
pub mod trace;

pub use scenario::{Cut, Edge, NodeSpec, Scenario, Send, Timing, Topology};
pub use sim::{SimError, run, run_with};
pub use trace::{NodeState, SCHEMA, Trace};
