//! A reliable byte stream over a [`Link`]: RNS `Channel`/`Buffer` framing plus link-proof
//! acknowledgement, driven sans-io.
//!
//! This is the piece that makes an `AsyncRead`/`AsyncWrite` link honest on a lossy medium.
//! Over TCP the medium already never drops, so [`endpoint`](crate::endpoint) keeps its
//! best-effort stream as the default; a caller opts into this reliable path for LoRa or
//! serial, where packets drop, reorder, and delay (mode-gated, mirroring RNS, whose Channel
//! is likewise opt-in over raw link data).
//!
//! Everything here is sans-io and composes the pieces already pinned to RNS 1.3.8's wire:
//!
//! - [`Buffer`] chunks bytes into `Channel` envelopes with a windowed 16-bit sequence and a
//!   receiver-side reorder buffer (`channel.rs`, gold-tested against `channel_wire.json` /
//!   `buffer_wire.json`).
//! - Each envelope rides a link data packet under context [`CTX_CHANNEL`], sealed with the
//!   link keys.
//! - The **ack is the link packet proof** ([`Link::data_proof`] / [`Link::verify_data_proof`],
//!   gold-tested against `rns_link_proof.json`): a received packet is proved back, and an
//!   inbound proof names the packet it acknowledges by hash, releasing that sequence.
//!
//! The driver holds a `full_hash -> sequence` map so a returning proof — addressed to the
//! link, carrying the proven packet's hash — resolves to the outstanding sequence it frees.
//! It is driven by a caller (a link task): [`poll_transmit`](ReliableChannel::poll_transmit)
//! with the clock yields packets to send (new data within the window, plus retransmits);
//! [`on_data_packet`](ReliableChannel::on_data_packet) feeds a received channel packet in and
//! returns the proof to send back; [`on_proof`](ReliableChannel::on_proof) feeds a received
//! proof in. That is exactly the shape a virtual-clock loss test drives (see the tests), so
//! the reliable path is validated on the desk before any radio exists.

use alloc::vec::Vec;

use heapless::index_map::FnvIndexMap;

use crate::capacity::desktop;
use crate::channel::{Buffer, Envelope, MAX_DATA_LEN};
use crate::hash::AddressHash;
use crate::identity::{Identity, PrivateIdentity};
use crate::link::{CTX_CHANNEL, Link};
use crate::packet::Packet;
use crate::token::IV_LEN;

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
    /// Our identity — signs the proofs of packets we receive.
    prover: PrivateIdentity,
    /// The peer's identity — validates the proofs of packets we sent. `None` until it is
    /// known: an initiator holds the destination's identity from the announce; a responder
    /// learns the initiator's from the IDENTIFY it sends ([`on_identify`](Self::on_identify)).
    /// Until it is set, the peer's proofs cannot be validated, so nothing we send is released.
    peer: Option<Identity>,
    /// Full hash of each channel packet we put on the wire, to its sequence. An inbound
    /// proof carries the hash; this maps it back to the sequence to release.
    ///
    /// One sequence can hold several entries at once, because a retransmit re-seals under a
    /// fresh IV and so arrives on the wire as a new hash. They are all released together
    /// when the sequence is proved.
    sent: FnvIndexMap<[u8; 32], u16, SENT>,
    /// Packets whose hash the table was too full to record. Not an error: an unrecorded
    /// packet is simply not releasable by its proof, so the buffer's retransmit timer fires
    /// and the packet goes out again under a fresh hash, which the table has room for once
    /// earlier sequences have been proved. Exposed so the degradation is visible rather than
    /// silent, per the plan's rule that a full table stays operational and says so.
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
    /// [`on_identify`](Self::on_identify)). Until then its proofs are not validated.
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
    pub fn on_identify(&mut self, packet: &Packet) -> bool {
        match self.link.read_identify(packet) {
            Some(peer) => {
                self.peer = Some(peer);
                true
            }
            None => false,
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
            // A retransmit re-seals under a fresh IV, so it reaches the wire as a new hash
            // for the same sequence. Both entries stay live until the sequence is proved,
            // because either packet's proof may be the one that returns.
            if self.sent.insert(packet.full_hash(), env.sequence).is_err() {
                self.unrecorded = self.unrecorded.saturating_add(1);
            }
            out.push(packet);
        }
        out
    }

    /// Feed an inbound channel data packet: decrypt and order its envelope, and return the
    /// PROOF to send back — the ack. A duplicate is still proved (the peer retransmitted
    /// because our earlier proof did not arrive); [`Buffer`] drops the duplicate payload.
    /// Returns `None` only if the packet does not decrypt or carries no valid envelope.
    pub fn on_data_packet(&mut self, packet: &Packet) -> Option<Packet> {
        let plaintext = self.link.decrypt(packet).ok()?;
        let envelope = Envelope::decode(&plaintext)?;
        // Prove only what we could accept. When the reorder buffer is full, `handle` returns
        // false: we withhold the proof so the sender retransmits later, rather than proving a
        // frame we dropped (which would lose it) — this is what bounds the reorder buffer.
        self.buffer
            .handle(envelope)
            .then(|| self.link.data_proof(packet, &self.prover))
    }

    /// Feed an inbound proof: if it validates against the peer's identity and names a packet
    /// we sent, release that sequence. Returns whether it matched an outstanding packet.
    pub fn on_proof(&mut self, proof: &Packet, now: u64) -> bool {
        let hash = match &self.peer {
            Some(peer) => self.link.verify_data_proof(proof, peer),
            None => None, // peer not yet identified: cannot validate its proofs
        };
        let Some(hash) = hash else {
            return false;
        };
        let Some(sequence) = self.sent.remove(&hash) else {
            return false;
        };
        // Sweep every other hash for this sequence. A retransmit leaves one entry each, and
        // only the proved one was just removed; without this the rest would sit in the table
        // for the life of the link. That is unbounded growth on a lossy medium, and it is
        // what bounding the table surfaced.
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

    /// Take all delivered, in-order application bytes.
    pub fn read(&mut self) -> Vec<u8> {
        self.buffer.read_available()
    }

    /// Whether the peer signalled end-of-stream.
    pub fn recv_finished(&mut self) -> bool {
        self.buffer.recv_finished()
    }

    /// Whether everything written has been sent and proven.
    pub fn send_idle(&self) -> bool {
        self.buffer.send_idle()
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
mod tests {
    use super::*;
    use crate::capacity::small_types::SmallReliableChannel;
    use crate::destination::DestinationName;
    use crate::link::{LinkMode, LinkTrailer, PendingLink, accept};
    use crate::lossy::LossModel;

    /// A client (initiator) and server (responder) reliable channel over one established
    /// link, each holding the other's identity for proof validation.
    fn pair() -> (ReliableChannel, ReliableChannel) {
        pair_bounded(None)
    }

    /// The same pair at a caller-chosen table size and send window, for exercising a full
    /// table. The window matters: it starts at [`crate::channel::WINDOW_INITIAL`] and only
    /// opens on sustained proofs, so a short transfer finishes before enough packets are
    /// outstanding to fill anything.
    fn pair_bounded<
        const N: usize,
        const W: usize,
        const Q: usize,
        const R: usize,
        const B: usize,
    >(
        max_window: Option<u32>,
    ) -> (
        ReliableChannel<N, W, Q, R, B>,
        ReliableChannel<N, W, Q, R, B>,
    ) {
        let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
        let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };
        let dest = DestinationName::new("retinue", ["test"]).destination_hash(server_id.public());
        let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x33; 64], trailer);
        let (responder_link, proof) = accept(&request, &server_id, &[0x99; 64], trailer).unwrap();
        let initiator_link = pending.prove(&proof).unwrap();

        match max_window {
            None => (
                ReliableChannel::new(initiator_link, client_id.clone(), *server_id.public()),
                ReliableChannel::new(responder_link, server_id, *client_id.public()),
            ),
            Some(window) => (
                ReliableChannel::new_with_initial_rtt_and_max_window(
                    initiator_link,
                    client_id.clone(),
                    *server_id.public(),
                    10,
                    window,
                ),
                ReliableChannel::new_with_initial_rtt_and_max_window(
                    responder_link,
                    server_id,
                    *client_id.public(),
                    10,
                    window,
                ),
            ),
        }
    }

    /// The same pair at the board profile. Inference picks the parameters off the return
    /// type, so the small profile is named once, in `capacity`.
    fn small_pair() -> (SmallReliableChannel, SmallReliableChannel) {
        pair_bounded(None)
    }

    fn counting_iv(counter: &mut u64) -> impl FnMut() -> [u8; IV_LEN] + '_ {
        move || {
            *counter += 1;
            let mut v = [0u8; IV_LEN];
            v[..8].copy_from_slice(&counter.to_le_bytes());
            v
        }
    }

    /// A proof releases every hash recorded for its sequence, not only the hash it names.
    ///
    /// A retransmit re-seals under a fresh IV, so one sequence reaches the wire as several
    /// hashes. Until this swept them, the generations that were dropped stayed in the table
    /// for the life of the link: unbounded growth on exactly the lossy medium this module
    /// exists to survive. Bounding the table is what surfaced it.
    #[test]
    fn a_proof_sweeps_every_hash_for_its_sequence() {
        let (mut client, mut server) = pair();
        assert_eq!(
            client.write(b"one small message"),
            b"one small message".len(),
            "the send queue took every byte"
        );
        client.finish();
        let mut ivc = 0u64;
        let mut iv = counting_iv(&mut ivc);

        // The first generation reaches the wire and is dropped on the floor, so its hashes
        // are recorded and no proof will ever name them.
        let dropped = client.poll_transmit(0, &mut iv);
        assert!(
            !dropped.is_empty(),
            "the first generation must reach the wire"
        );
        assert_eq!(client.sent.len(), dropped.len());

        // Now deliver and prove everything until the send side goes idle.
        for now in 1..2_000_000u64 {
            for packet in client.poll_transmit(now, &mut iv) {
                if let Some(proof) = server.on_data_packet(&packet) {
                    client.on_proof(&proof, now);
                }
            }
            if client.send_idle() {
                break;
            }
        }

        assert!(client.send_idle(), "the stream must complete");
        assert_eq!(
            client.sent.len(),
            0,
            "hashes from the dropped generation outlived their proved sequence"
        );
        assert_eq!(
            client.unrecorded(),
            0,
            "nothing overflowed at the desktop size"
        );
    }

    /// A table too small for the window holds its bound, keeps putting packets on the wire,
    /// and counts what it could not record.
    ///
    /// Nothing is proved here, so the table fills and stays full. That is the interesting
    /// state: the plan's rule is that a full table keeps serving and says so, rather than
    /// stalling or growing. An unrecorded packet is simply not releasable by its proof, so
    /// the retransmit timer carries it instead, which costs airtime and not correctness.
    #[test]
    fn a_full_table_holds_its_bound_and_counts_the_overflow() {
        let (mut client, _server) =
            pair_bounded::<2, 64, 256, 256, 65_536>(Some(crate::channel::WINDOW_MAX));
        assert_eq!(
            client.write(&[7u8; 4_000]),
            4_000,
            "the send queue took every byte"
        );
        client.finish();
        let mut ivc = 0u64;
        let mut iv = counting_iv(&mut ivc);

        let mut reached_the_wire = 0usize;
        for now in 0..5_000u64 {
            reached_the_wire += client.poll_transmit(now, &mut iv).len();
        }

        assert!(
            reached_the_wire > 2,
            "packets must keep reaching the wire past the table size, not stop at it"
        );
        assert!(
            client.sent.len() <= 2,
            "the table grew past its bound: {}",
            client.sent.len()
        );
        assert!(
            client.unrecorded() > 0,
            "the refusals must be counted, not silent"
        );
    }

    /// The board profile carries a stream end to end, running the same code the desktop
    /// runs at different table sizes.
    ///
    /// This is the point of parameterising rather than forking. If a board had its own
    /// windowing or reassembly, the desktop would stop being an oracle for it and would
    /// become a second implementation that merely interoperates.
    #[test]
    fn the_small_profile_carries_a_stream_end_to_end() {
        let (mut client, mut server) = small_pair();
        let payload: Vec<u8> = (0..3_000u32).map(|i| (i.wrapping_mul(17)) as u8).collect();

        let mut offset = 0;
        let mut ivc = 0u64;
        let mut iv = counting_iv(&mut ivc);
        let mut got = Vec::new();
        let mut finished = false;

        for now in 0..200_000u64 {
            // The board's queue is shallow, so the writer feeds it as room appears. A short
            // write here is the expected case, not a failure.
            if offset < payload.len() {
                offset += client.write(&payload[offset..]);
            } else if !finished {
                finished = client.finish();
            }
            for packet in client.poll_transmit(now, &mut iv) {
                if let Some(proof) = server.on_data_packet(&packet) {
                    client.on_proof(&proof, now);
                }
            }
            got.extend(server.read());
            if finished && client.send_idle() && server.recv_finished() {
                break;
            }
        }

        assert_eq!(
            got, payload,
            "the small profile must carry the bytes exactly"
        );
        assert!(client.send_idle(), "and must drain its queue");
    }

    /// Drive `client`'s payload to `server` over a lossy pipe on a virtual clock: channel
    /// packets forward (subject to loss), proofs back (subject to loss), retransmits on the
    /// clock. Asserts exact, in-order reconstruction and that the server saw eof.
    fn drive_over_loss(drop_per_mille: u32, max_delay: u64, seed: u64, len: usize) {
        let (mut client, mut server) = pair();
        let payload: Vec<u8> = (0..len as u32)
            .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
            .collect();
        assert_eq!(
            client.write(&payload),
            payload.len(),
            "the send queue took every byte"
        );
        client.finish();

        let mut fwd = LossModel::new(seed)
            .drop_per_mille(drop_per_mille)
            .max_delay_ms(max_delay);
        let mut bwd = LossModel::new(seed ^ 0xABCD)
            .drop_per_mille(drop_per_mille)
            .max_delay_ms(max_delay);

        let mut to_server: Vec<(u64, Packet)> = Vec::new();
        let mut to_client: Vec<(u64, Packet)> = Vec::new();
        let mut got: Vec<u8> = Vec::new();
        let mut ivc: u64 = 0;

        for now in 0..2_000_000u64 {
            let mut iv = || {
                ivc += 1;
                let mut v = [0u8; IV_LEN];
                v[..8].copy_from_slice(&ivc.to_le_bytes());
                v
            };
            for pkt in client.poll_transmit(now, &mut iv) {
                if !fwd.should_drop() {
                    to_server.push((now + 1 + fwd.delay_ms(), pkt));
                }
            }
            let mut still = Vec::new();
            for (t, pkt) in core::mem::take(&mut to_server) {
                if t <= now {
                    if let Some(proof) = server.on_data_packet(&pkt)
                        && !bwd.should_drop()
                    {
                        to_client.push((now + 1 + bwd.delay_ms(), proof));
                    }
                } else {
                    still.push((t, pkt));
                }
            }
            to_server = still;
            to_client.retain(|(t, proof)| {
                if *t <= now {
                    client.on_proof(proof, now);
                    false
                } else {
                    true
                }
            });
            got.extend(server.read());
            if got.len() == payload.len() && client.send_idle() {
                break;
            }
        }
        assert_eq!(
            got, payload,
            "reliable stream must reconstruct exactly over loss"
        );
        assert!(server.recv_finished(), "server saw the client's eof");
    }

    #[test]
    fn reliable_stream_is_faithful_without_loss() {
        drive_over_loss(0, 0, 1, 5000);
    }

    #[test]
    fn reliable_stream_survives_drop() {
        drive_over_loss(300, 0, 7, 5000);
    }

    #[test]
    fn reliable_stream_survives_drop_reorder_and_delay() {
        drive_over_loss(250, 6, 42, 4000);
    }

    #[test]
    fn reliable_stream_survives_heavy_loss() {
        drive_over_loss(600, 3, 99, 3000);
    }

    #[test]
    fn a_forged_proof_releases_nothing() {
        // A proof signed by the wrong identity, or naming a packet we never sent, must not
        // release an outstanding sequence.
        let (mut client, mut server) = pair();
        assert_eq!(
            client.write(b"one small message that fits in a single channel packet"),
            b"one small message that fits in a single channel packet".len(),
            "the send queue took every byte"
        );
        let mut ivc = 0u64;
        let mut iv = || {
            ivc += 1;
            let mut v = [0u8; IV_LEN];
            v[..8].copy_from_slice(&ivc.to_le_bytes());
            v
        };
        let sent = client.poll_transmit(0, &mut iv);
        assert!(!sent.is_empty());
        server.on_data_packet(&sent[0]).unwrap();

        // A proof from a stranger's identity over the right hash: rejected (wrong signer).
        let stranger = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
        let forged = client.link.data_proof(&sent[0], &stranger);
        assert!(
            !client.on_proof(&forged, 1),
            "wrong-identity proof rejected"
        );
        assert!(!client.send_idle(), "the packet is still outstanding");

        // The genuine proof (server signs with its identity) does release it.
        let real = server.on_data_packet(&sent[0]).unwrap();
        assert!(client.on_proof(&real, 2), "genuine proof accepted");
    }

    #[test]
    fn a_responder_validates_proofs_only_after_identify() {
        // A responder starts without the initiator's identity, so it cannot validate the
        // initiator's proofs of the data the responder sends. After the initiator's IDENTIFY,
        // it learns the identity and the same proof is accepted.
        let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
        let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };
        let dest = DestinationName::new("retinue", ["test"]).destination_hash(server_id.public());
        let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x33; 64], trailer);
        let (responder_link, proof) = accept(&request, &server_id, &[0x99; 64], trailer).unwrap();
        let initiator_link = pending.prove(&proof).unwrap();

        // Server accepts without knowing the client; the client already knows the server.
        let server_pub = *server_id.public();
        let mut server: ReliableChannel = ReliableChannel::accepting(responder_link, server_id);
        let mut client: ReliableChannel =
            ReliableChannel::new(initiator_link, client_id.clone(), server_pub);

        // The server sends a message; the client receives it and proves it back.
        assert_eq!(
            server.write(b"a message from the server"),
            b"a message from the server".len(),
            "the send queue took every byte"
        );
        let mut ivc = 0u64;
        let mut iv = || {
            ivc += 1;
            let mut v = [0u8; IV_LEN];
            v[..8].copy_from_slice(&ivc.to_le_bytes());
            v
        };
        let sent = server.poll_transmit(0, &mut iv);
        assert!(!sent.is_empty());
        let proof = client
            .on_data_packet(&sent[0])
            .expect("client proves the server's packet");

        // Before identify the server cannot validate the client's proof, so it is not released.
        assert!(server.peer().is_none(), "no peer yet");
        assert!(
            !server.on_proof(&proof, 1),
            "proof rejected before identify"
        );
        assert!(
            !server.send_idle(),
            "the server's packet is still outstanding"
        );

        // The client identifies; now the server learns it and accepts the same proof.
        let id_packet = client.link.identify_packet(&client_id, &[0x07; IV_LEN]);
        assert!(server.on_identify(&id_packet), "server learns the client");
        assert_eq!(
            server.peer().map(|p| p.hash()),
            Some(client_id.public().hash())
        );
        assert!(server.on_proof(&proof, 2), "proof accepted after identify");
        assert!(server.send_idle(), "the server's packet is now released");
    }
}
