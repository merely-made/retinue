//! Outrider: LXMF as a boundary crate in the retinue family.
//!
//! An outrider rides ahead of or beside the party to scout the road and carry
//! word. This crate carries mail for the household: an implementation of LXMF,
//! the message format and delivery system of the
//! [Reticulum](https://reticulum.network/) ecosystem, riding on
//! [retinue](https://github.com/merely-made/retinue)'s destinations, links, and
//! resources. Not affiliated with the Reticulum or LXMF projects.
//!
//! Outrider is a boundary crate: a codec, delivery state machines, and a
//! propagation client/server, consumed by an application at its edge. It does
//! not define conversation, contact, or storage semantics; those belong to
//! the consumer.
//!
//! # Status
//!
//! The message codec, ratcheted opportunistic lane, stamped direct-delivery
//! lane, propagation submit/fetch client, and bounded in-memory propagation
//! server are interoperable with pinned LXMF 0.9.6 / RNS 1.4.2 black-box
//! oracles. Node peering remains open. Propagation-store state is versioned
//! and caller-persisted.
//! The founding scope, provenance discipline, and ordered gates live in
//! `design_docs/2026-07-25_outrider_lxmf_founding.md` in the repository.
//!
//! # Provenance
//!
//! Outrider is implemented from the public LXMF specification prose and
//! black-box observation of pinned stock clients. The Python LXMF
//! implementation and its client applications are not implementation inputs. See
//! `PROVENANCE.md`.
//!
//! # Building for a board
//!
//! The delivery, propagation and announce modules want a host: `rmpv`'s value tree,
//! retinue's endpoint, its TCP interface. `default-features = false` turns all of that off
//! and leaves [`portable`] and [`stamp`], which is firmware's shape: enough to read a
//! message, know its identity, and mint or check the proof-of-work on it.

// `not(test)` because a test harness needs `std` even when the crate under test does not.
// Today the arm is unreachable: the dev-dependency on postilion depends back on outrider
// with default features, so a test build always has `std` on. Which is the trap worth
// naming, since it makes `cargo test --no-default-features` look like a `no_std` receipt
// when it is an ordinary test run. Only a build for a target without `std` proves that.
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]

extern crate alloc;

/// The `no_std` codec, built beside the shipping one. See its own docs for the bar it
/// must clear before it replaces `codec`.
pub mod portable;
/// Proof-of-work stamps. `no_std`, so a board can mint and check its own.
pub mod stamp;

#[cfg(feature = "std")]
pub mod announce;
#[cfg(feature = "std")]
pub mod codec;
#[cfg(feature = "std")]
pub mod direct;
#[cfg(feature = "std")]
pub mod opportunistic;
#[cfg(feature = "std")]
pub mod propagation;

#[cfg(not(feature = "std"))]
pub use portable::{CodecError, DESTINATION_LEN, HEADER_LEN, SIGNATURE_LEN, SOURCE_LEN};

#[cfg(feature = "std")]
pub use announce::{
    AnnounceError, DEFAULT_MAX_ANNOUNCE_BYTES, DeliveryAnnounce, delivery_destination,
    delivery_name, resolve_source, resolve_source_with_link,
};
#[cfg(feature = "std")]
pub use codec::{
    CodecError, DEFAULT_MAX_MESSAGE_BYTES, DESTINATION_LEN, DecodedLxmf, HEADER_LEN, LxmfPayload,
    PreparedLxmf, SIGNATURE_LEN, SOURCE_LEN, decode, decode_bounded, prepare,
};
#[cfg(feature = "std")]
pub use direct::{
    DirectError, DirectReceipt, ReceivedDirect, announce as announce_delivery,
    receive as receive_direct, receive_with_resource_config as receive_direct_with_resource_config,
    receive_with_stamp_cost as receive_direct_with_stamp_cost,
    receive_with_stamp_cost_and_resource_config as receive_direct_with_stamp_cost_and_resource_config,
    register as register_delivery, send as send_direct, send_stamped as send_direct_stamped,
    send_stamped_with_resource_config as send_direct_stamped_with_resource_config,
    send_with_resource_config as send_direct_with_resource_config,
};
#[cfg(feature = "std")]
pub use opportunistic::{
    OpportunisticError, OpportunisticReceipt, ReceivedOpportunistic,
    receive as receive_opportunistic,
    receive_with_stamp_cost as receive_opportunistic_with_stamp_cost,
    register as register_opportunistic, send as send_opportunistic,
    send_stamped as send_opportunistic_stamped,
};
#[cfg(feature = "std")]
pub use propagation::{
    DEFAULT_MAX_PROPAGATION_ANNOUNCE_BYTES, DEFAULT_MAX_PROPAGATION_BATCH_BYTES,
    DEFAULT_MAX_PROPAGATION_ENTRIES, DEFAULT_MAX_PROPAGATION_STORE_SNAPSHOT_BYTES,
    DEFAULT_MAX_STORED_MESSAGE_BYTES, FETCH_LIMIT, FETCH_PATH_HASH, FetchedPropagation,
    MIN_ENCRYPTED_MESSAGE_BYTES, PROPAGATION_METADATA_NAME, PreparedPropagation,
    PropagationAnnounce, PropagationBatch, PropagationCosts, PropagationEntry, PropagationError,
    PropagationFetchReceipt, PropagationMessage, PropagationStore, PropagationStoreLimits,
    PropagationSubmitReceipt, ReceivedPropagationBatch, ServedFetch, StoreReceipt,
    StoreRestoreReceipt, announce_propagation, fetch as fetch_propagation,
    fetch_with_resource_config as fetch_propagation_with_resource_config, prepare_propagation,
    propagation_destination, propagation_name, receive_submission, register_propagation,
    serve_fetch, submit as submit_propagation,
    submit_with_resource_config as submit_propagation_with_resource_config,
};
pub use stamp::{
    MESSAGE_WORKBLOCK_ROUNDS, PROPAGATION_WORKBLOCK_ROUNDS, STAMP_LEN, WORKBLOCK_BYTES_PER_ROUND,
    find as find_stamp, find_streamed as find_stamp_streamed, propagation_valid,
    propagation_value, valid as stamp_valid, value as stamp_value,
    value_streamed as stamp_value_streamed, workblock,
};
