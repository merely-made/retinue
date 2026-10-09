//! A reliable byte stream over a [`Link`]: RNS `Channel`/`Buffer` framing plus link-proof
//! acknowledgement, driven sans-io.
//!
//! TCP never drops, so [`endpoint`](crate::endpoint) keeps a best-effort stream by default;
//! LoRa or serial opt into this path, as RNS's Channel is opt-in over raw link data. Wire
//! fixtures were captured with RNS 1.3.8:
//!
//! - [`Buffer`] carries bytes in sequenced `Channel` envelopes (`channel_wire.json`,
//!   `buffer_wire.json`), each in a link data packet under [`CTX_CHANNEL`].
//! - The **ack is the link packet proof** ([`Link::prove_packet`] / [`Link::validate_proof`],
//!   `rns_link_proof.json`): a proof names the packet it acknowledges by hash, and a
//!   `full_hash -> sequence` map turns it back into the sequence it releases.
//!
//! A link task drives it: [`poll_transmit`](ReliableChannel::poll_transmit) yields packets to
//! send, [`on_data_packet`](ReliableChannel::on_data_packet) takes a channel packet and
//! returns its proof, and [`on_proof`](ReliableChannel::on_proof) takes a returning proof.

use alloc::vec::Vec;

use heapless::index_map::FnvIndexMap;

use crate::capacity::desktop;
use crate::channel::{Buffer, Envelope, MAX_DATA_LEN};
use crate::hash::AddressHash;
use crate::identity::{Identity, PrivateIdentity};
use crate::link::{CTX_CHANNEL, Link};
use crate::packet::Packet;
use crate::token::IV_LEN;

/// The largest link RTT an initiator's RTT packet may set, in milliseconds. RNS trusts the
/// report; a cap keeps a peer from disabling retransmission and its give-up.
const MAX_REPORTED_RTT_MS: u64 = 60_000;

/// A reliable, in-order byte stream over one [`Link`]. See the module docs.
///
/// `SENT` bounds the hash table below. It defaults to
/// [`capacity::desktop::SENT_HASHES`](crate::capacity::desktop::SENT_HASHES), so
/// writing the bare type gets the desktop profile; a board writes
/// `ReliableChannel<{ capacity::small::SENT_HASHES }>`.
pub struct ReliableChannel<
    const SENT: usize = { desktop::SENT_HASHES },
    const WINDOW: usize = 64,
    const QUEUE: usize = 256,
    const REORDER: usize = { crate::channel::REORDER_MAX },
    const READ_BYTES: usize = 65_536,
> {
    link: Link,
    buffer: Buffer<WINDOW, QUEUE, REORDER, READ_BYTES>,
    /// Our identity. Proofs are signed with the link's own key, not this; it is kept so an
    /// echo of our own IDENTIFY is never taken for the peer's.
    prover: PrivateIdentity,
    /// The peer's identity. `None` until it is known: an initiator holds the destination's
    /// identity from the announce; a responder learns the initiator's from the IDENTIFY it
    /// sends ([`on_identify`](Self::on_identify)). Proofs are validated against the link's
    /// peer key without it; an IDENTIFY'd identity is also accepted as a proof key, for older
    /// retinue initiators that signed with their long-term identity.
    peer: Option<Identity>,
    /// Full hash of each channel packet we sent, to its sequence. A retransmit re-seals under
    /// a fresh IV, so one sequence can hold several entries; all are released when it is proved.
    sent: FnvIndexMap<[u8; 32], u16, SENT>,
    /// Packets whose hash the table was too full to record. Not an error: the retransmit
    /// timer resends them under a fresh hash. Counted per the plan's rule that a full table
    /// stays operational and says so.
    unrecorded: u32,
}

impl<
    const SENT: usize,
    const WINDOW: usize,
    const QUEUE: usize,
    const REORDER: usize,
    const READ_BYTES: usize,
> ReliableChannel<SENT, WINDOW, QUEUE, REORDER, READ_BYTES>
{
    /// A reliable channel whose peer is already known — an initiator, holding the
    /// destination's identity from its announce. `prover` is our identity.
    pub fn new(link: Link, prover: PrivateIdentity, peer: Identity) -> Self {
        Self::build(link, prover, Some(peer), None, None)
    }

    /// Initiator with a medium-specific first RTT estimate, in milliseconds.
    pub fn new_with_initial_rtt(
        link: Link,
        prover: PrivateIdentity,
        peer: Identity,
        initial_rtt_ms: u64,
    ) -> Self {
        Self::build(link, prover, Some(peer), Some(initial_rtt_ms), None)
    }

    /// Initiator with medium-specific RTT and maximum in-flight frame count.
    pub fn new_with_initial_rtt_and_max_window(
        link: Link,
        prover: PrivateIdentity,
        peer: Identity,
        initial_rtt_ms: u64,
        max_window: u32,
    ) -> Self {
        Self::build(
            link,
            prover,
            Some(peer),
            Some(initial_rtt_ms),
            Some(max_window),
        )
    }

    /// A reliable channel whose peer is not yet known — a responder, which learns the
    /// initiator's identity from the IDENTIFY it sends (feed packets to
    /// [`on_identify`](Self::on_identify)). Its proofs validate against the link's peer key
    /// whether or not it identifies.
    pub fn accepting(link: Link, prover: PrivateIdentity) -> Self {
        Self::build(link, prover, None, None, None)
    }

    /// Responder with a medium-specific first RTT estimate, in milliseconds.
    pub fn accepting_with_initial_rtt(
        link: Link,
        prover: PrivateIdentity,
        initial_rtt_ms: u64,
    ) -> Self {
        Self::build(link, prover, None, Some(initial_rtt_ms), None)
    }

    /// Responder with medium-specific RTT and maximum in-flight frame count.
    pub fn accepting_with_initial_rtt_and_max_window(
        link: Link,
        prover: PrivateIdentity,
        initial_rtt_ms: u64,
        max_window: u32,
    ) -> Self {
        Self::build(link, prover, None, Some(initial_rtt_ms), Some(max_window))
    }

    fn build(
        link: Link,
        prover: PrivateIdentity,
        peer: Option<Identity>,
        initial_rtt_ms: Option<u64>,
        max_window: Option<u32>,
    ) -> Self {
        // Type-1 link header + token framing + CBC padding + Channel/Stream headers.
        // This reproduces RNS's 423-byte default at MTU 500 and shrinks when a radio
        // endpoint negotiates a smaller link MTU.
        let token_room = (link.mtu() as usize).saturating_sub(crate::packet::HEADER_MIN_LEN);
        let cipher_room = token_room.saturating_sub(crate::token::TOKEN_OVERHEAD);
        let padded_plain = (cipher_room / 16) * 16;
        let max_chunk = padded_plain
            .saturating_sub(1)
            .saturating_sub(6 + 2)
            .clamp(1, MAX_DATA_LEN);
        Self {
            link,
            buffer: match (initial_rtt_ms, max_window) {
                (Some(rtt), Some(window)) => Buffer::with_policy(rtt, window, max_chunk),
                (Some(rtt), None) => {
                    Buffer::with_policy(rtt, crate::channel::WINDOW_MAX, max_chunk)
                }
                (None, _) => Buffer::with_max_chunk(max_chunk),
            },
            prover,
            peer,
            sent: FnvIndexMap::new(),
            unrecorded: 0,
        }
    }

    /// Feed an inbound IDENTIFY packet: if it validates, learn the peer identity so the
    /// peer's proofs can be validated from here on. Returns whether it was learned.
    ///
    /// Only the first identity is learned, and never our own. On a shared medium our own
    /// IDENTIFY comes back from a relay under the shared link key with a valid signature;
    /// taking it would make us our own peer and fail every real proof. An initiator already
    /// holds its peer from the announce, so it learns nothing here.
    pub fn on_identify(&mut self, packet: &Packet) -> bool {
        if self.peer.is_some() {
            return false;
        }
        match self.link.read_identify(packet) {
            Some(peer) if peer != *self.prover.public() => {
                self.peer = Some(peer);
                true
            }
            _ => false,
        }
    }

    /// The peer identity, once known.
    pub fn peer(&self) -> Option<&Identity> {
        self.peer.as_ref()
    }

    /// Queue application bytes for reliable, in-order delivery.
    ///
    /// Returns how many were accepted; a short count means the send queue is full and the
    /// caller should retry the rest after [`poll_transmit`](Self::poll_transmit) drains it.
    #[must_use]
    pub fn write(&mut self, bytes: &[u8]) -> usize {
        self.buffer.write(bytes)
    }

    /// Mark our send stream finished with an end-of-stream frame. Returns whether it was
    /// queued; a full send queue refuses it and the caller retries.
    pub fn finish(&mut self) -> bool {
        self.buffer.finish()
    }

    /// The channel packets to put on the wire at time `now`: newly sendable envelopes within
    /// the window and retransmits past their timeout, each sealed under [`CTX_CHANNEL`].
    /// `iv` supplies a fresh IV per packet (it must not repeat for the link key). Each
    /// packet's hash is recorded so its returning proof releases the right sequence.
    pub fn poll_transmit(&mut self, now: u64, mut iv: impl FnMut() -> [u8; IV_LEN]) -> Vec<Packet> {
        let mut out = Vec::new();
        for env in self.buffer.poll_transmit(now) {
            let packet = self.link.sealed_packet(CTX_CHANNEL, &env.encode(), &iv());
            // Every hash for a sequence stays live until it is proved: either packet's proof
            // may be the one that returns.
            if self.sent.insert(packet.full_hash(), env.sequence).is_err() {
                self.unrecorded = self.unrecorded.saturating_add(1);
            }
            out.push(packet);
        }
        // A failed channel sends nothing more, so no recorded hash can be proved.
        if self.buffer.channel_error().is_some() {
            self.sent.clear();
        }
        out
    }

    /// Feed an inbound channel data packet: decrypt and order its envelope, and return the
    /// PROOF to send back — the ack. A duplicate is still proved (the peer retransmitted
    /// because our earlier proof did not arrive); [`Buffer`] drops the duplicate payload.
    /// Returns `None` for invalid packets, backpressure, or a terminal receive
    /// decode error. A prior queued packet may already have been proved before
    /// its decode failure becomes visible; callers must inspect [`Self::receive_error`].
    pub fn on_data_packet(&mut self, packet: &Packet) -> Option<Packet> {
        let plaintext = self.link.decrypt(packet).ok()?;
        let envelope = Envelope::decode(&plaintext)?;
        // Prove only what we could accept: a proved, dropped frame would be lost.
        if !self.buffer.handle(envelope) {
            return None;
        }
        // Admission can precede decoding when earlier bytes fill the read queue.
        // Suppress the proof whenever the terminal failure is already discoverable
        // here; any previously admitted packet may already have been proved.
        self.buffer
            .receive_error()
            .is_none()
            .then(|| self.link.prove_packet(packet))
    }

    /// Feed an inbound proof: if it validates against the peer's link key and names a packet
    /// we sent, release that sequence. Returns whether it matched an outstanding packet.
    pub fn on_proof(&mut self, proof: &Packet, now: u64) -> bool {
        // Transitional: an older retinue initiator proves with its long-term identity, which
        // a responder only knows once that initiator has sent IDENTIFY.
        let hash = self.link.validate_proof(proof).or_else(|| {
            self.peer
                .as_ref()
                .and_then(|peer| self.link.verify_data_proof(proof, peer))
        });
        let Some(hash) = hash else {
            return false;
        };
        let Some(sequence) = self.sent.remove(&hash) else {
            return false;
        };
        // Sweep the sequence's other hashes (one per retransmit), or they stay for the life
        // of the link.
        self.sent.retain(|_, outstanding| *outstanding != sequence);
        self.buffer.on_proof(sequence, now);
        true
    }

    /// Packets whose hash did not fit the table, so their proof cannot release them and the
    /// retransmit timer has to. Zero on a healthy link; a climbing count means `SENT` is
    /// too small for the window and loss rate, and airtime is being spent on it.
    pub fn unrecorded(&self) -> u32 {
        self.unrecorded
    }

    /// Take up to the buffer's read bound of delivered, in-order application bytes.
    /// Repeat until empty to drain an expanded compressed frame.
    pub fn read(&mut self) -> Vec<u8> {
        self.buffer.read_available()
    }

    /// Sticky terminal receive error, including an oversized compressed frame.
    /// Previously decoded bytes remain readable; receive EOF stays false.
    pub fn receive_error(&mut self) -> Option<crate::channel::StreamDecodeError> {
        self.buffer.receive_error()
    }

    /// Configure the per-frame decoded output ceiling before receiving data.
    pub fn set_decoded_frame_limit(
        &mut self,
        limit: usize,
    ) -> Result<(), crate::channel::StreamDecodeLimitError> {
        self.buffer.set_decoded_frame_limit(limit)
    }

    /// Whether the peer signalled end-of-stream and all received bytes were read.
    pub fn recv_finished(&mut self) -> bool {
        self.buffer.recv_finished()
    }

    /// Whether everything written has been sent and proven.
    pub fn send_idle(&self) -> bool {
        self.buffer.send_idle()
    }

    /// Feed the link's RTT packet (context [`CTX_LRRTT`](crate::link::CTX_LRRTT)), which an
    /// initiator sends a responder right after the proof. Like RNS, take the larger of the
    /// RTT it reports and `measured`, the responder's own time from proof to this packet, and
    /// start the channel's timeouts from that rather than a guess. Both in milliseconds.
    /// Returns whether the packet decrypted to an RTT.
    pub fn on_rtt_packet(&mut self, packet: &Packet, measured: u64) -> bool {
        let Ok(plain) = self.link.decrypt(packet) else {
            return false;
        };
        // RNS packs a MessagePack float: float64 (0xcb) from Python, float32 (0xca) allowed.
        let seconds = match plain.split_first() {
            Some((0xcb, b)) => b.try_into().map(f64::from_be_bytes).ok(),
            Some((0xca, b)) => b.try_into().map(f32::from_be_bytes).ok().map(f64::from),
            _ => None,
        };
        let Some(seconds) = seconds.filter(|s| s.is_finite() && *s >= 0.0) else {
            return false;
        };
        // The peer controls this value: cap it, so a huge report cannot push every deadline
        // out of reach and keep the channel from ever giving up.
        let reported = ((seconds * 1000.0) as u64).min(MAX_REPORTED_RTT_MS);
        self.buffer.set_initial_rtt(reported.max(measured));
        true
    }

    /// Set how many times one channel packet may go on the wire before the channel fails
    /// (RNS's limit, [`DEFAULT_MAX_TRIES`](crate::channel::DEFAULT_MAX_TRIES), by default).
    pub fn set_max_tries(&mut self, tries: u8) {
        self.buffer.set_max_tries(tries);
    }

    /// Why the channel stopped, once it has: a packet went unproved through every try. The
    /// link is dead then, and the caller should close it, as RNS tears its link down.
    pub fn channel_error(&self) -> Option<crate::channel::ChannelError> {
        self.buffer.channel_error()
    }

    /// The current send window (diagnostics).
    pub fn window(&self) -> u32 {
        self.buffer.window()
    }

    /// The id of the link this stream rides.
    pub fn link_id(&self) -> AddressHash {
        self.link.id()
    }
}

#[cfg(test)]
mod tests;
