//! A reliable, in-order message layer over a link — RNS `Channel`, wire-compatible.
//!
//! Link data packets are best-effort: TCP never drops them, but LoRa or serial drop,
//! reorder, and delay. This layer makes a stream honest on any medium with sequence
//! numbers, a send window, retransmission of unproven packets, and receiver reordering.
//!
//! The wire is RNS 1.3.8's, captured black-box (`design_docs/2026-07-13_rns_wire_format_reference.md`
//! §3.9; fixtures `channel_wire.json` / `channel_link.json`):
//!
//! - A message is an [`Envelope`], `[msgtype u16][sequence u16][length u16][payload]`
//!   big-endian, in a link data packet with context `14` (`0x0e`).
//! - The sequence is windowed **16-bit** (mod [`SEQ_MODULUS`]).
//! - **Acknowledgement is the link packet proof, not an ack message.** Each envelope packet
//!   requests a proof; an unproven sequence is retransmitted.
//!
//! [`Channel`] is **sans-io**: no sockets, no clock. A link driver calls
//! [`poll_transmit`](Channel::poll_transmit) with the time, feeds received envelopes to
//! [`handle`](Channel::handle), and maps returning proofs to [`on_proof`](Channel::on_proof),
//! so loss and reordering are testable on a virtual clock (see `retinue::lossy`).

mod buffer;
mod recv;
mod send;
mod state;
mod window;
mod wire;

#[cfg(test)]
mod tests;

pub use buffer::{
    Buffer, DEFAULT_CHUNK, DEFAULT_DECODED_FRAME_LIMIT, StreamDecodeError, StreamDecodeLimitError,
};
pub use state::{Channel, ChannelError, REORDER_MAX};
pub use window::{
    DEFAULT_MAX_TRIES, DEFAULT_RETX_TIMEOUT, WINDOW_FLEXIBILITY, WINDOW_INITIAL, WINDOW_MAX,
    WINDOW_MIN,
};
pub use wire::{Envelope, MAX_DATA_LEN, SEQ_MODULUS, STREAM_ID_MAX, STREAM_MSGTYPE, StreamFrame};
