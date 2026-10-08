//! A reliable, in-order message layer over a link — RNS `Channel`, wire-compatible.
//!
//! Raw link data packets are best-effort by spec: over TCP they never drop, so an
//! `AsyncRead`/`AsyncWrite` link is reliable *by accident of the medium*. Over LoRa
//! or serial they drop, reorder, and delay, and a stream that returns `Ok` for bytes
//! that never arrive is lying to its caller. This is the layer that makes the stream
//! honest on any medium: sequence numbers, a send window, retransmission of unproven
//! packets, and receiver-side reordering.
//!
//! The wire is RNS 1.3.8's, captured black-box (see
//! `design_docs/2026-07-13_rns_wire_format_reference.md` §3.9, fixtures
//! `channel_wire.json` / `channel_link.json`):
//!
//! - A message is an [`Envelope`] — `[msgtype u16][sequence u16][length u16][payload]`,
//!   big-endian — carried in a link data packet with context `14` (`0x0e`).
//! - The sequence is windowed **16-bit** (mod [`SEQ_MODULUS`]).
//! - **Acknowledgement is the link packet proof, not an ack message.** Each envelope
//!   packet is proof-requesting; an unproven sequence is retransmitted. Confirmed by
//!   capture: RNS resent a seq-0 envelope 5 times while the receiver stayed silent.
//!
//! It is **sans-io**: [`Channel`] holds no sockets and no clock. The caller (a link
//! driver) drives it — [`poll_transmit`](Channel::poll_transmit) with the current
//! time yields the envelopes to put on the wire (new data within the window, plus
//! retransmits past their timeout); [`handle`](Channel::handle) feeds received
//! envelopes back in; and [`on_proof`](Channel::on_proof) releases an outstanding
//! sequence when its packet's proof arrives (the driver maps proof-by-packet-hash to
//! sequence). That is exactly what makes the retransmit and reorder paths testable
//! against a deterministic loss model on a virtual clock (see `retinue::lossy`).

use alloc::vec::Vec;

use alloc::collections::VecDeque;

use heapless::Deque;
use heapless::index_map::FnvIndexMap;

/// The sequence space: sequences are 16-bit and wrap at this modulus (RNS
/// `SEQ_MODULUS`). Comparisons use wrapping distance with a half-modulus split to
/// tell "ahead" (a future packet to buffer) from "behind" (an old duplicate).
pub const SEQ_MODULUS: u32 = 65536;

/// Dynamic send-window constants, from RNS's `Channel` (1.5.7 `Channel.py` 200-245). The
/// window bounds unacknowledged envelopes in flight. It grows by one per proof up to
/// `window_max`, and `window_max` itself is promoted to the next RTT tier after
/// [`FAST_RATE_THRESHOLD`] proofs below that tier's RTT; each timeout shrinks the window by
/// one. It is a *local* send-rate policy, never on the wire, so matching RNS's tiers is a
/// tuning choice, interoperable either way. `new` starts at [`WINDOW_INITIAL`].
pub const WINDOW_INITIAL: u32 = 2;
/// The window never shrinks below this (RNS `WINDOW_MIN`), unless the link starts slower
/// than the slow RTT tier, when RNS pins the whole window to one.
pub const WINDOW_MIN: u32 = 2;
/// The window never grows above this (RNS `WINDOW_MAX`, the fast-tier ceiling).
pub const WINDOW_MAX: u32 = 48;
/// The smallest gap a timeout leaves between `window_max` and `window_min` (RNS
/// `WINDOW_FLEXIBILITY`): a timeout lowers `window_max` by one only while it is more than
/// this far above the floor. It is not a step size; the window itself drops by one.
pub const WINDOW_FLEXIBILITY: u32 = 4;

const WINDOW_MAX_SLOW: u32 = 5;
const WINDOW_MAX_MEDIUM: u32 = 12;
const WINDOW_MAX_FAST: u32 = 48;
const WINDOW_MIN_LIMIT_MEDIUM: u32 = 5;
const WINDOW_MIN_LIMIT_FAST: u32 = 16;
// RTT tier thresholds, in the caller's tick unit. RNS's are seconds; these read a tick
// as a millisecond (RNS RTT_FAST/MEDIUM/SLOW = 0.18 / 0.75 / 1.45 s).
const RTT_FAST: u64 = 180;
const RTT_MEDIUM: u64 = 750;
const RTT_SLOW: u64 = 1450;
/// Proofs measured below a tier's RTT before `window_max` is promoted to that tier (RNS
/// `FAST_RATE_THRESHOLD`).
const FAST_RATE_THRESHOLD: u32 = 10;

/// Ticks without a proof before an outstanding envelope is retransmitted, for a fixed
/// channel ([`Channel::with_params`]). "Tick" is whatever unit the caller passes to
/// [`Channel::poll_transmit`] (milliseconds over a real clock; a counter in tests).
pub const DEFAULT_RETX_TIMEOUT: u64 = 4;

/// How many times one envelope goes on the wire before the channel gives up (RNS
/// `Channel._max_tries`). The first send is a try, so this is the transmission count.
pub const DEFAULT_MAX_TRIES: u8 = 5;

/// The floor of the per-envelope timeout's RTT term, in ticks read as milliseconds (RNS
/// `max(rtt * 2.5, 0.025)`).
const RETX_RTT_FLOOR: u64 = 25;

/// How long an envelope on its `tries`-th transmission waits for its proof, with
/// `outstanding` envelopes in flight, given an RTT estimate. RNS `_get_packet_timeout_time`:
/// `1.5^(tries-1) * max(2.5 * rtt, 25 ms) * (outstanding + 1.5)`, in integer ticks. The
/// backoff exponent stops at the default try count, so a raised limit cannot overflow it.
fn retx_timeout(rtt: u64, tries: u8, outstanding: usize) -> u64 {
    let backoff = u32::from(tries.clamp(1, DEFAULT_MAX_TRIES) - 1);
    let base = (rtt.saturating_mul(5) / 2).max(RETX_RTT_FLOOR);
    base.saturating_mul(2 * outstanding as u64 + 3)
        .saturating_mul(3u64.pow(backoff))
        / (2u64 << backoff)
}

/// Why a channel stopped. Terminal: once set, nothing more is sent and the link should be
/// torn down, as RNS does when a channel times out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChannelError {
    /// An envelope went unproved through every try (RNS tears the link down here).
    RetriesExhausted {
        /// The sequence that was never proved.
        sequence: u16,
    },
}

/// The most out-of-order future envelopes the receiver will hold at once. A well-behaved
/// sender keeps at most a window's worth in flight (`WINDOW_MAX` = 48), so this is generous
/// headroom; its purpose is to bound the reorder buffer against a peer that streams only
/// future sequences and never fills the gap. See [`Channel::handle`].
pub const REORDER_MAX: usize = 256;

/// RNS `Buffer`'s stream-frame message type: a stream chunk rides a [`Channel`]
/// envelope under this msgtype (RNS `StreamDataMessage.MSGTYPE`). Captured black-box
/// (`buffer_wire.json`).
pub const STREAM_MSGTYPE: u16 = 0xFF00;

/// The largest stream id. The id is the low 14 bits of the [`StreamFrame`] header (RNS
/// `StreamDataMessage.STREAM_ID_MAX`); the top two bits are the eof / compressed flags.
pub const STREAM_ID_MAX: u16 = 0x3FFF;

/// The most stream data bytes in one [`StreamFrame`] (RNS `StreamDataMessage.MAX_DATA_LEN`):
/// the link MDU less the 6-byte envelope header and the 2-byte stream header (`OVERHEAD` 8).
pub const MAX_DATA_LEN: usize = 423;

/// One channel message on the wire: `[msgtype u16][sequence u16][length u16][payload]`,
/// big-endian. This is RNS 1.3.8's `Channel.Envelope` layout exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    /// Registered message type (identifies the message class on the wire).
    pub msgtype: u16,
    /// Windowed 16-bit sequence number.
    pub sequence: u16,
    /// Message payload.
    pub payload: Vec<u8>,
}

impl Envelope {
    /// Encode to the RNS wire layout.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(6 + self.payload.len());
        out.extend_from_slice(&self.msgtype.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Decode from the RNS wire layout, or `None` if malformed / the declared length
    /// does not match.
    pub fn decode(bytes: &[u8]) -> Option<Envelope> {
        let msgtype = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?);
        let sequence = u16::from_be_bytes(bytes.get(2..4)?.try_into().ok()?);
        let length = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
        let payload = bytes.get(6..6 + length)?.to_vec();
        Some(Envelope {
            msgtype,
            sequence,
            payload,
        })
    }
}

/// One un-acknowledged outbound envelope.
struct Outstanding {
    payload: Vec<u8>,
    /// When it last went on the wire.
    last_tx: u64,
    /// When it is next due for a retransmit (or, out of tries, for giving up).
    deadline: u64,
    /// How many times it has gone on the wire (RNS `Envelope.tries`).
    tries: u8,
}

/// A reliable, in-order message channel. See the module docs.
///
/// The three parameters bound its tables. They default to the desktop profile, so writing
/// the bare type is unchanged; a board writes [`SmallChannel`](crate::capacity::small_types::SmallChannel).
///
/// - `WINDOW` caps in-flight envelopes. It also caps the protocol send window, which is
///   clamped to it at construction, so the window can never outrun its own table.
/// - `QUEUE` bounds the application-facing queues in each direction.
/// - `REORDER` bounds held-back future sequences, defaulting to [`REORDER_MAX`].
///
/// Payloads stay heap-allocated, so an entry costs a `Vec` header rather than
/// [`MAX_DATA_LEN`] bytes and an idle channel stays cheap. What is bounded here is how many
/// entries exist, which is what grows without limit on a lossy medium.
pub struct Channel<
    const WINDOW: usize = 64,
    const QUEUE: usize = 256,
    const REORDER: usize = REORDER_MAX,
> {
    msgtype: u16,
    window: u32,
    /// Current growth ceiling (RNS `window_max`): promoted by RTT tier, lowered by timeouts.
    window_max: u32,
    /// Current shrink floor (RNS `window_min`).
    window_min: u32,
    /// Smallest gap timeouts leave between `window_max` and `window_min`.
    window_flexibility: u32,
    /// Caller-selected ceiling over every tier. A value of one serializes data/proof turns
    /// on strict half-duplex media.
    max_window: u32,
    /// The fixed retransmit timeout of a [`with_params`](Self::with_params) channel.
    retx_timeout: u64,
    /// Whether the window and timeout adapt. Off for a fixed window (`with_params`).
    dynamic: bool,
    /// Consecutive proofs measured inside the fast / medium RTT tier.
    fast_rate_rounds: u32,
    medium_rate_rounds: u32,
    /// EWMA of the proof round-trip, in the caller's tick unit; selects the RTT tier.
    rtt: u64,
    /// Whether `rtt` is measured yet, or still the caller's initial guess.
    rtt_measured: bool,
    /// Transmissions per envelope before the channel fails.
    max_tries: u8,
    /// Set once the channel has given up; see [`error`](Self::error).
    error: Option<ChannelError>,

    // ── send side ──
    /// Application payloads not yet assigned a sequence (waiting for window room).
    outgoing: Deque<Vec<u8>, QUEUE>,
    /// In-flight, unacknowledged, keyed by sequence. Released by [`on_proof`].
    outstanding: FnvIndexMap<u16, Outstanding, WINDOW>,
    /// The next sequence to assign (wraps at `SEQ_MODULUS`).
    send_next: u16,

    // ── receive side ──
    /// The next sequence we can deliver in order.
    recv_next: u16,
    /// Received-but-not-yet-deliverable, held until the gap before them fills.
    ///
    /// Keyed rather than ordered: delivery pulls `recv_next` by exact key, so this never
    /// iterates in sequence order and does not need an ordered map.
    reorder: FnvIndexMap<u16, (u16, Vec<u8>), REORDER>,
    /// Delivered, in order, ready for the application to read, each with its msgtype.
    inbox: Deque<(u16, Vec<u8>), QUEUE>,
}

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize> Default
    for Channel<WINDOW, QUEUE, REORDER>
{
    fn default() -> Self {
        Self::new(STREAM_MSGTYPE)
    }
}

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize>
    Channel<WINDOW, QUEUE, REORDER>
{
    /// A channel for one message type with a **dynamic** window: it starts at
    /// [`WINDOW_INITIAL`] and grows toward the RTT tier's max on sustained proofs,
    /// shrinking on timeouts. The first RTT estimate is the medium tier's bound.
    pub fn new(msgtype: u16) -> Self {
        Self::with_initial_rtt(msgtype, RTT_MEDIUM)
    }

    /// A dynamic channel whose first retransmit estimate is tuned to the selected medium.
    /// Subsequent proofs still adapt the estimate from measured round-trip time.
    pub fn with_initial_rtt(msgtype: u16, initial_rtt: u64) -> Self {
        Self::with_initial_rtt_and_max_window(msgtype, initial_rtt, WINDOW_MAX)
    }

    /// A dynamic channel with medium-specific RTT and send-window policy.
    ///
    /// As in RNS, a link starting slower than the slow tier (1450 ticks) runs a window of
    /// one; otherwise the window starts at [`WINDOW_INITIAL`] under the slow tier's ceiling
    /// and is promoted from measured RTT.
    ///
    /// `max_window = 1` is useful for strict half-duplex radios: the sender waits
    /// for each proof before transmitting the next frame, so a receiver's proof
    /// cannot collide with a second in-flight data frame.
    pub fn with_initial_rtt_and_max_window(
        msgtype: u16,
        initial_rtt: u64,
        max_window: u32,
    ) -> Self {
        let mut channel = Self::with_params(msgtype, max_window, 0);
        channel.start_window(initial_rtt);
        channel.dynamic = true;
        channel.rtt = initial_rtt;
        channel
    }

    /// RNS's starting window for a link of round trip `rtt`: one when slower than the slow
    /// tier, else [`WINDOW_INITIAL`] under the slow tier's ceiling.
    fn start_window(&mut self, rtt: u64) {
        let (window, window_max, window_min, flexibility) = if rtt > RTT_SLOW {
            (1, 1, 1, 1)
        } else {
            (
                WINDOW_INITIAL,
                WINDOW_MAX_SLOW,
                WINDOW_MIN,
                WINDOW_FLEXIBILITY,
            )
        };
        // The profile's table caps the protocol window as well as the protocol's own
        // WINDOW_MAX, so a small board cannot be talked into a window its table cannot hold.
        let ceiling = self.max_window;
        self.window = window.min(ceiling);
        self.window_max = window_max.min(ceiling);
        self.window_min = window_min.min(ceiling);
        self.window_flexibility = flexibility;
    }

    /// A channel with a **fixed** window and explicit retransmit timeout (for tests and
    /// callers that want a static send rate). The try limit still applies.
    pub fn with_params(msgtype: u16, window: u32, retx_timeout: u64) -> Self {
        let window = window.clamp(1, WINDOW_MAX.min(WINDOW as u32).max(1));
        Self {
            msgtype,
            window,
            window_max: window,
            window_min: window,
            window_flexibility: WINDOW_FLEXIBILITY,
            max_window: window,
            retx_timeout,
            dynamic: false,
            fast_rate_rounds: 0,
            medium_rate_rounds: 0,
            rtt: RTT_MEDIUM,
            rtt_measured: false,
            max_tries: DEFAULT_MAX_TRIES,
            error: None,
            outgoing: Deque::new(),
            outstanding: FnvIndexMap::new(),
            send_next: 0,
            recv_next: 0,
            reorder: FnvIndexMap::new(),
            inbox: Deque::new(),
        }
    }

    /// Set how many times one envelope may go on the wire before the channel fails
    /// (default [`DEFAULT_MAX_TRIES`], at least one). The timeout backoff stops growing at
    /// the default try count, so a larger limit adds evenly spaced tries.
    pub fn set_max_tries(&mut self, tries: u8) {
        self.max_tries = tries.max(1);
    }

    /// Replace the RTT estimate with a link-level measurement, such as the handshake RTT RNS
    /// times its channel by, until a proof measures the round trip itself. Ignored once a
    /// proof has, and on a fixed channel. Deadlines already set stay as they are.
    ///
    /// Before anything has been sent this also redoes the starting window from `rtt`, so a
    /// responder built on a guess before the link's RTT arrived still gets RNS's window of
    /// one on a slow link (RNS creates its channel only once the link is active).
    pub fn set_initial_rtt(&mut self, rtt: u64) {
        if self.dynamic && !self.rtt_measured {
            self.rtt = rtt;
            if self.send_next == 0 && self.outstanding.is_empty() {
                self.start_window(rtt);
            }
        }
    }

    /// Why the channel stopped, once it has. A failed channel sends nothing more, refuses
    /// new data, and is never [`send_idle`](Self::send_idle); its link should be closed.
    pub fn error(&self) -> Option<ChannelError> {
        self.error
    }

    /// The current send window.
    pub fn window(&self) -> u32 {
        self.window
    }

    /// The current RTT estimate, in ticks (diagnostics).
    pub fn rtt(&self) -> u64 {
        self.rtt
    }

    /// Queue a payload for reliable, in-order delivery. Assigned a sequence and put on
    /// the wire by [`poll_transmit`](Self::poll_transmit) as the window allows.
    /// Returns the payload back when the send queue is full, or the channel has failed, so
    /// a caller that writes faster than the link drains is told rather than silently
    /// growing the queue forever.
    pub fn send(&mut self, payload: Vec<u8>) -> Result<(), Vec<u8>> {
        if self.error.is_some() {
            return Err(payload);
        }
        self.outgoing.push_back(payload)
    }

    /// Whether [`send`](Self::send) has room. Application-facing backpressure.
    pub fn send_room(&self) -> usize {
        QUEUE - self.outgoing.len()
    }

    /// The envelopes to transmit at time `now`: retransmissions of outstanding envelopes
    /// past their timeout, then newly sendable data within the window. There is no ack
    /// envelope — acknowledgement is the link proof, delivered via
    /// [`on_proof`](Self::on_proof).
    ///
    /// An envelope that times out on its last try fails the channel instead (see
    /// [`error`](Self::error)): this returns nothing then or after.
    pub fn poll_transmit(&mut self, now: u64) -> Vec<Envelope> {
        let mut out = Vec::new();
        if self.error.is_some() {
            return out;
        }

        // Retransmit anything unproved past its deadline, or give up on it (RNS
        // `_packet_timeout`). Each timeout steps the window down by one.
        let mut timeouts = 0u32;
        let mut exhausted = None;
        for (&seq, o) in &mut self.outstanding {
            if now < o.deadline {
                continue;
            }
            if o.tries >= self.max_tries {
                exhausted = Some(seq);
                break;
            }
            o.tries += 1;
            o.last_tx = now;
            o.deadline = 0; // recomputed below, once the in-flight count is settled
            out.push(Envelope {
                msgtype: self.msgtype,
                sequence: seq,
                payload: o.payload.clone(),
            });
            timeouts += 1;
        }
        if let Some(sequence) = exhausted {
            self.fail(ChannelError::RetriesExhausted { sequence });
            return Vec::new();
        }
        if self.dynamic {
            for _ in 0..timeouts {
                if self.window > self.window_min {
                    self.window -= 1;
                    if self.window_max > self.window_min + self.window_flexibility {
                        self.window_max -= 1;
                    }
                }
            }
        }

        // Fill the window with fresh data. The window is clamped to WINDOW at construction,
        // so the table has room, but the guard keeps that a local fact rather than an
        // assumption about a constructor three functions away.
        //
        // The window also bounds the sequence *span*, not just the count. An RNS receiver
        // drops any envelope more than WINDOW_MAX past its next expected sequence, yet the
        // link has already proved it, so the sender retires data that was never delivered.
        // Counting only unproved envelopes lets a lost one fall far behind while proved
        // successors keep the count low. A new sequence is assigned only while it stays
        // within WINDOW_MAX of the oldest unproved one. The oldest cannot change inside
        // this loop: everything assigned here is newer.
        let oldest = self.oldest_outstanding();
        while (self.outstanding.len() as u32) < self.window
            && !self.outstanding.is_full()
            && u32::from(self.send_next.wrapping_sub(oldest)) < WINDOW_MAX
        {
            let Some(payload) = self.outgoing.pop_front() else {
                break;
            };
            let seq = self.send_next;
            self.send_next = self.send_next.wrapping_add(1);
            out.push(Envelope {
                msgtype: self.msgtype,
                sequence: seq,
                payload: payload.clone(),
            });
            let record = Outstanding {
                payload,
                last_tx: now,
                deadline: 0,
                tries: 1,
            };
            if let Err((_, unsent)) = self.outstanding.insert(seq, record) {
                // Unreachable while the guard above holds. Put the payload back and undo the
                // sequence rather than dropping application data on the floor.
                self.send_next = self.send_next.wrapping_sub(1);
                out.pop();
                let _ = self.outgoing.push_front(unsent.payload);
                break;
            }
        }

        // Anything sent sets its own deadline, and every in-flight envelope's deadline may
        // only move later now that more share the link (RNS `_update_packet_timeouts`).
        if !out.is_empty() {
            let in_flight = self.outstanding.len();
            let (dynamic, rtt, fixed) = (self.dynamic, self.rtt, self.retx_timeout);
            for o in self.outstanding.values_mut() {
                let timeout = if dynamic {
                    retx_timeout(rtt, o.tries, in_flight)
                } else {
                    fixed
                };
                o.deadline = o.deadline.max(o.last_tx.saturating_add(timeout));
            }
        }

        out
    }

    /// Give up: record why and drop everything unsent or unproved (RNS `_shutdown`).
    fn fail(&mut self, error: ChannelError) {
        self.error = Some(error);
        self.outstanding.clear();
        self.outgoing.clear();
    }

    /// The oldest unproved sequence, or `send_next` when nothing is in flight. Age is the
    /// wrapping distance back from `send_next`, so this holds across the 16-bit wrap.
    fn oldest_outstanding(&self) -> u16 {
        self.outstanding
            .keys()
            .copied()
            .max_by_key(|&seq| self.send_next.wrapping_sub(seq))
            .unwrap_or(self.send_next)
    }

    /// Release an outstanding sequence: its packet's proof arrived. Selective — RNS
    /// proves each packet individually, so this frees exactly one sequence. `now` lets
    /// the dynamic window measure RTT.
    ///
    /// Each proof opens the window by one up to `window_max`, and [`FAST_RATE_THRESHOLD`]
    /// proofs inside a faster RTT tier promote `window_max` and `window_min` to that tier
    /// (RNS `_packet_tx_op`). RTT is sampled only from envelopes sent once (Karn's rule): a
    /// proof of a retransmitted envelope cannot say which transmission it answers. The first
    /// sample replaces the initial estimate; later samples are averaged in.
    pub fn on_proof(&mut self, sequence: u16, now: u64) {
        let Some(o) = self.outstanding.remove(&sequence) else {
            return;
        };
        if !self.dynamic {
            return;
        }
        if o.tries == 1 {
            // The first sample replaces the initial guess outright, as RNS starts from the
            // link's measured handshake RTT rather than a guess; later ones are smoothed.
            let sample = now.saturating_sub(o.last_tx);
            self.rtt = if self.rtt_measured {
                (self.rtt * 7 + sample) / 8
            } else {
                sample
            };
            self.rtt_measured = true;
        }
        if self.window < self.window_max {
            self.window += 1;
        }
        let ceiling = self.max_window;
        if self.rtt > RTT_FAST {
            self.fast_rate_rounds = 0;
            if self.rtt > RTT_MEDIUM {
                self.medium_rate_rounds = 0;
            } else {
                self.medium_rate_rounds = self.medium_rate_rounds.saturating_add(1);
                if self.window_max < WINDOW_MAX_MEDIUM
                    && self.medium_rate_rounds == FAST_RATE_THRESHOLD
                {
                    self.window_max = WINDOW_MAX_MEDIUM.min(ceiling);
                    self.window_min = WINDOW_MIN_LIMIT_MEDIUM.min(ceiling);
                }
            }
        } else {
            self.fast_rate_rounds = self.fast_rate_rounds.saturating_add(1);
            if self.window_max < WINDOW_MAX_FAST && self.fast_rate_rounds == FAST_RATE_THRESHOLD {
                self.window_max = WINDOW_MAX_FAST.min(ceiling);
                self.window_min = WINDOW_MIN_LIMIT_FAST.min(ceiling);
            }
        }
    }

    /// Process a received envelope, delivering it or buffering it for reordering.
    ///
    /// Returns whether the driver should prove (acknowledge) the underlying packet. It
    /// proves in-order and buffered frames, and re-proves duplicates (an unproven sender
    /// retransmits). It withholds the proof only when the reorder buffer is full and this is
    /// a new gap-filler: dropping a *proved* frame would lose it forever, so instead we leave
    /// it unproven and let the sender retransmit once the gap ahead of it clears. This bounds
    /// the reorder buffer against a peer that streams only future sequences.
    #[must_use]
    pub fn handle(&mut self, envelope: Envelope) -> bool {
        let ahead = envelope.sequence.wrapping_sub(self.recv_next);
        if ahead == 0 {
            // A full inbox means the application is not reading. Withhold the proof for the
            // same reason a full reorder buffer does: an unproved frame is retransmitted,
            // where a proved-then-dropped frame is lost. This is the backpressure path.
            if self.inbox.is_full() {
                return false;
            }
            let _ = self.inbox.push_back((envelope.msgtype, envelope.payload));
            self.recv_next = self.recv_next.wrapping_add(1);
            // Pull any now-contiguous buffered envelopes into order, stopping if the inbox
            // fills so the rest stay held rather than dropped.
            self.pump();
            true
        } else if (ahead as u32) < SEQ_MODULUS / 2 {
            // A future sequence within the forward half of the space: hold it, unless the
            // reorder buffer is full of other gap-fillers and this is a new one.
            if self.reorder.is_full() && !self.reorder.contains_key(&envelope.sequence) {
                return false;
            }
            let _ = self
                .reorder
                .entry(envelope.sequence)
                .or_insert((envelope.msgtype, envelope.payload));
            true
        } else {
            // Behind `recv_next`: an already-delivered duplicate. Drop the payload but prove
            // it — the sender retransmitted because our earlier proof did not arrive.
            true
        }
    }

    /// Move contiguous frames from the reorder buffer into the inbox while there is room.
    ///
    /// This must run on the *read* path as well as the receive path. A frame buffered out
    /// of order was proved when it arrived, so the sender will never retransmit it; if the
    /// inbox fills mid-drain, that frame is stranded in `reorder` with `recv_next` pointing
    /// at it, and no future arrival carries `recv_next` to re-trigger the drain in
    /// [`handle`](Self::handle). Only the application making room can free it, so the pump
    /// runs when the application reads.
    fn pump(&mut self) {
        while !self.inbox.is_full() {
            let Some(next) = self.reorder.remove(&self.recv_next) else {
                break;
            };
            let _ = self.inbox.push_back(next);
            self.recv_next = self.recv_next.wrapping_add(1);
        }
    }

    /// The next in-order application payload, if one is ready, whatever its msgtype.
    /// Use [`recv_message`](Self::recv_message) to dispatch on the type.
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        self.recv_message().map(|(_, payload)| payload)
    }

    /// The next in-order message as `(msgtype, payload)`, if one is ready.
    ///
    /// Every message is sequenced and proved whatever its type, so an unexpected type
    /// never stalls the sequence; deciding what a type means is the reader's business.
    pub fn recv_message(&mut self) -> Option<(u16, Vec<u8>)> {
        // Pump before popping: if the inbox is empty but proved frames sit in the reorder
        // buffer (see `pump`), this is the moment they become deliverable. Pumping first
        // also means this never returns `None` while in-order data is stranded.
        self.pump();
        self.inbox.pop_front()
    }

    /// Whether everything queued to send has been sent and proven.
    pub fn send_idle(&self) -> bool {
        self.error.is_none() && self.outgoing.is_empty() && self.outstanding.is_empty()
    }

    /// Count of in-flight, unproven envelopes.
    pub fn in_flight(&self) -> usize {
        self.outstanding.len()
    }
}

/// One RNS `Buffer` stream frame: the payload of the [`Channel`] envelope carrying a
/// stream chunk. Layout `[u16 BE header][data]`, header = `eof<<15 | compressed<<14 |
/// stream_id` (`stream_id` in the low 14 bits, [`STREAM_ID_MAX`]). The data length is
/// implied by the enclosing envelope's length field, so the frame carries none. This is
/// RNS 1.3.8's `StreamDataMessage.pack()` layout exactly (captured in `buffer_wire.json`).
///
/// `compressed` marks a bz2 transform applied to `data` *before* framing, not a layout
/// change — `pack()` stores `data` verbatim either way. Retinue never sets it on send;
/// [`Buffer`] decodes it on receive when the `compression` feature is enabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamFrame {
    /// Stream id (14-bit): which multiplexed stream this chunk belongs to.
    pub stream_id: u16,
    /// End-of-stream marker: the last frame of this stream.
    pub eof: bool,
    /// Whether `data` is bz2-compressed (see the type docs).
    pub compressed: bool,
    /// The stream bytes (uncompressed unless `compressed`).
    pub data: Vec<u8>,
}

impl StreamFrame {
    const EOF_BIT: u16 = 0x8000;
    const COMPRESSED_BIT: u16 = 0x4000;

    /// Encode to the RNS stream-frame layout.
    pub fn encode(&self) -> Vec<u8> {
        let mut header = self.stream_id & STREAM_ID_MAX;
        if self.eof {
            header |= Self::EOF_BIT;
        }
        if self.compressed {
            header |= Self::COMPRESSED_BIT;
        }
        let mut out = Vec::with_capacity(2 + self.data.len());
        out.extend_from_slice(&header.to_be_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Decode from the RNS stream-frame layout, or `None` if shorter than the header.
    pub fn decode(bytes: &[u8]) -> Option<StreamFrame> {
        let header = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?);
        Some(StreamFrame {
            stream_id: header & STREAM_ID_MAX,
            eof: header & Self::EOF_BIT != 0,
            compressed: header & Self::COMPRESSED_BIT != 0,
            data: bytes.get(2..)?.to_vec(),
        })
    }
}

/// Default per-frame chunk: RNS's own `MAX_DATA_LEN` — the most stream bytes that fit in
/// one link data packet after the envelope and stream headers.
pub const DEFAULT_CHUNK: usize = MAX_DATA_LEN;

/// Default maximum decoded bytes in one compressed stream frame. This is separate
/// from `READ_BYTES`, which only bounds the ready-to-read queue.
pub const DEFAULT_DECODED_FRAME_LIMIT: usize = 65_536;

/// A terminal stream receive failure. Once set, the buffer delivers only bytes
/// decoded before the bad frame and never reports a clean receive EOF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDecodeError {
    /// This build has no bz2 decoder.
    UnsupportedCompression,
    /// The bz2 data is malformed.
    InvalidCompression,
    /// Decoded bytes exceeded the configured per-frame output ceiling.
    DecodedFrameLimitExceeded { limit: usize },
}

/// Invalid decoded-frame configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDecodeLimitError {
    /// The limit must be positive and leave room for one sentinel byte.
    InvalidLimit,
}

/// A byte stream over a reliable [`Channel`], RNS `Buffer`-wire-compatible. Each write
/// chunk is a [`StreamFrame`] (stream id + eof + data) carried in a [`Channel`] envelope
/// under [`STREAM_MSGTYPE`]; [`read`](Self::read) concatenates delivered frames' data in
/// order. The stream-shaped, reliable face of `Channel` — the piece an
/// `AsyncRead + AsyncWrite` link binds to once a driver pumps
/// [`poll_transmit`](Self::poll_transmit) / [`handle`](Self::handle) /
/// [`on_proof`](Self::on_proof) against the wire. Sans-io like `Channel`.
///
/// The stream is multiplexable: a buffer sends on `send_stream_id` and reads only frames
/// tagged with `recv_stream_id`, so one `Channel` carries several streams (RNS's
/// bidirectional buffer is two ids over one channel). [`finish`](Self::finish) marks the
/// send stream done with an eof frame; [`recv_finished`](Self::recv_finished) reports the
/// peer's eof.
///
/// A buffer reads only [`STREAM_MSGTYPE`] envelopes. A message of any other type is still
/// sequenced and proved, so the stream does not stall behind it, and is then dropped:
/// its bytes never reach the reader and cannot set eof or a receive error. A caller that
/// wants other message types uses a [`Channel`] and
/// [`recv_message`](Channel::recv_message) directly.
///
/// Retinue does not compress sent frames. With the `compression` feature, received
/// compressed frames are decoded before delivery. Decode failure is terminal and
/// reported by [`receive_error`](Self::receive_error); it is never a healthy EOF.
pub struct Buffer<
    const WINDOW: usize = 64,
    const QUEUE: usize = 256,
    const REORDER: usize = REORDER_MAX,
    const READ_BYTES: usize = 65_536,
> {
    channel: Channel<WINDOW, QUEUE, REORDER>,
    max_chunk: usize,
    send_stream_id: u16,
    recv_stream_id: u16,
    /// Bytes decoded from delivered frames, awaiting the reader.
    ///
    /// Heap-backed and bounded at runtime against `READ_BYTES`, rather than a `Deque<u8,
    /// READ_BYTES>`, which would commit that many bytes of static storage per link even
    /// while idle. [`fill`](Self::fill) stops draining the channel once this is at its
    /// bound. A decoded frame that does not fit waits in `pending_frame`; only one such
    /// frame is retained before pressure reaches the bounded inbox and withheld proofs.
    read_buf: VecDeque<u8>,
    /// Remaining bytes from one delivered frame. Decompression still allocates the full
    /// decoded frame; `READ_BYTES` bounds the ready-to-read queue, not that allocation.
    pending_frame: Option<(Vec<u8>, usize)>,
    recv_eof: bool,
    decoded_frame_limit: usize,
    receive_error: Option<StreamDecodeError>,
}

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize, const READ_BYTES: usize> Default
    for Buffer<WINDOW, QUEUE, REORDER, READ_BYTES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize, const READ_BYTES: usize>
    Buffer<WINDOW, QUEUE, REORDER, READ_BYTES>
{
    /// A buffer with the default channel and chunk size, stream id 0 both ways.
    pub fn new() -> Self {
        Self::with_channel(Channel::new(STREAM_MSGTYPE), DEFAULT_CHUNK)
    }

    /// A default dynamic channel with an explicit application chunk ceiling.
    pub fn with_max_chunk(max_chunk: usize) -> Self {
        Self::with_channel(Channel::new(STREAM_MSGTYPE), max_chunk)
    }

    /// A buffer whose channel starts with a medium-specific RTT estimate.
    pub fn with_initial_rtt(initial_rtt: u64) -> Self {
        Self::with_channel(
            Channel::with_initial_rtt(STREAM_MSGTYPE, initial_rtt),
            DEFAULT_CHUNK,
        )
    }

    /// A buffer whose dynamic channel is capped for the selected medium.
    pub fn with_initial_rtt_and_max_window(initial_rtt: u64, max_window: u32) -> Self {
        Self::with_policy(initial_rtt, max_window, DEFAULT_CHUNK)
    }

    /// A buffer with explicit RTT, dynamic-window ceiling, and application chunk size.
    pub fn with_policy(initial_rtt: u64, max_window: u32, max_chunk: usize) -> Self {
        Self::with_channel(
            Channel::with_initial_rtt_and_max_window(STREAM_MSGTYPE, initial_rtt, max_window),
            max_chunk,
        )
    }

    /// A buffer over an explicit channel and chunk size, stream id 0 both ways.
    pub fn with_channel(channel: Channel<WINDOW, QUEUE, REORDER>, max_chunk: usize) -> Self {
        Self::with_streams(channel, max_chunk, 0, 0)
    }

    /// A buffer with explicit send / receive stream ids (each clamped to
    /// [`STREAM_ID_MAX`]) — one channel multiplexing distinct streams.
    pub fn with_streams(
        channel: Channel<WINDOW, QUEUE, REORDER>,
        max_chunk: usize,
        send_stream_id: u16,
        recv_stream_id: u16,
    ) -> Self {
        Self {
            channel,
            max_chunk: max_chunk.clamp(1, MAX_DATA_LEN),
            send_stream_id: send_stream_id & STREAM_ID_MAX,
            recv_stream_id: recv_stream_id & STREAM_ID_MAX,
            read_buf: VecDeque::new(),
            pending_frame: None,
            recv_eof: false,
            decoded_frame_limit: DEFAULT_DECODED_FRAME_LIMIT,
            receive_error: None,
        }
    }

    /// Set the decoded output ceiling for each compressed frame. Uncompressed
    /// frames are already bounded by the link packet and retain their prior behavior.
    /// `limit + 1` bytes are reserved for output and oversize detection; bz2's
    /// decoder workspace is additional. Configure before receiving packets.
    pub fn set_decoded_frame_limit(&mut self, limit: usize) -> Result<(), StreamDecodeLimitError> {
        if limit == 0 || limit >= isize::MAX as usize {
            return Err(StreamDecodeLimitError::InvalidLimit);
        }
        self.decoded_frame_limit = limit;
        Ok(())
    }

    /// The active per-frame decoded output ceiling.
    pub fn decoded_frame_limit(&self) -> usize {
        self.decoded_frame_limit
    }

    /// Queue bytes for reliable, in-order delivery, chunked into [`StreamFrame`]s.
    ///
    /// Returns how many bytes were accepted, which is fewer than `bytes.len()` when the
    /// send queue fills. A caller writing faster than the link drains is told so, rather
    /// than growing the queue without limit. Chunking means the split is always on a frame
    /// boundary, so a partial accept never tears a frame.
    #[must_use]
    pub fn write(&mut self, bytes: &[u8]) -> usize {
        let mut accepted = 0;
        for chunk in bytes.chunks(self.max_chunk) {
            if self.send_frame(chunk.to_vec(), false).is_err() {
                break;
            }
            accepted += chunk.len();
        }
        accepted
    }

    /// Mark the send stream finished: queue an empty eof frame. RNS also accepts eof
    /// riding a final data frame; a standalone eof is the simpler equivalent.
    ///
    /// Returns whether the eof frame was queued; a full send queue refuses it, and the
    /// caller retries once [`poll_transmit`](Self::poll_transmit) has drained room.
    pub fn finish(&mut self) -> bool {
        self.send_frame(Vec::new(), true).is_ok()
    }

    fn send_frame(&mut self, data: Vec<u8>, eof: bool) -> Result<(), ()> {
        let frame = StreamFrame {
            stream_id: self.send_stream_id,
            eof,
            compressed: false,
            data,
        };
        self.channel.send(frame.encode()).map_err(|_| ())
    }

    /// Copy up to `out.len()` delivered bytes into `out`, returning the count read.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        self.fill();
        let n = out.len().min(self.read_buf.len());
        for slot in out.iter_mut().take(n) {
            *slot = self.read_buf.pop_front().expect("len checked");
        }
        n
    }

    /// Take up to `READ_BYTES` currently-available delivered bytes.
    pub fn read_available(&mut self) -> Vec<u8> {
        self.fill();
        self.read_buf.drain(..).collect()
    }

    /// Recover a compressed stream frame within the configured output ceiling.
    ///
    /// Without the `compression` feature there is no bz2 decoder linked, so the honest
    /// answer is that these bytes are unreadable by this build. RNS compresses only when it
    /// shrinks the payload, so a peer that never compresses never reaches this at all.
    fn decompressed(&self, data: &[u8]) -> Result<Vec<u8>, StreamDecodeError> {
        #[cfg(feature = "compression")]
        {
            crate::resource::decompress_bounded(data, self.decoded_frame_limit).map_err(|e| match e
            {
                crate::resource::BoundedDecompressError::InvalidData => {
                    StreamDecodeError::InvalidCompression
                }
                crate::resource::BoundedDecompressError::LimitExceeded => {
                    StreamDecodeError::DecodedFrameLimitExceeded {
                        limit: self.decoded_frame_limit,
                    }
                }
            })
        }
        #[cfg(not(feature = "compression"))]
        {
            let _ = data;
            Err(StreamDecodeError::UnsupportedCompression)
        }
    }

    fn fill(&mut self) {
        if self.receive_error.is_some() {
            return;
        }
        // Stop draining once the reader is this far behind. At most one decoded frame
        // waits outside read_buf; later frames stay in the channel's bounded inbox.
        while self.read_buf.len() < READ_BYTES {
            if let Some((data, cursor)) = &mut self.pending_frame {
                let count = (READ_BYTES - self.read_buf.len()).min(data.len() - *cursor);
                self.read_buf
                    .extend(data[*cursor..*cursor + count].iter().copied());
                *cursor += count;
                if *cursor == data.len() {
                    self.pending_frame = None;
                }
                continue;
            }
            let Some((msgtype, msg)) = self.channel.recv_message() else {
                break;
            };
            if msgtype != STREAM_MSGTYPE {
                // Not stream data. The channel has already sequenced and proved it, so the
                // sequence moves on; a Buffer has no use for it and drops it here rather
                // than reading its bytes as a stream frame.
                continue;
            }
            let Some(frame) = StreamFrame::decode(&msg) else {
                continue; // malformed frame; the channel already ordered/deduped it
            };
            if frame.stream_id != self.recv_stream_id {
                continue; // a different multiplexed stream on the same channel
            }
            let data = if frame.compressed {
                // A compressed frame used to be counted as unsupported and thrown away. That
                // was silent data loss with a receipt on it: the reliable layer has already
                // proven this packet to the peer by the time the bytes get here, so the
                // sender retires data the application never sees, and nothing anywhere reads
                // the flag that recorded it. The crate has always been able to decompress
                // (`resource::decompress`, the same bz2 pass a compressed resource takes);
                // this path simply never called it.
                match self.decompressed(&frame.data) {
                    Ok(data) => Some(data),
                    Err(error) => {
                        self.receive_error = Some(error);
                        break;
                    }
                }
            } else {
                Some(frame.data)
            };
            if let Some(data) = data.filter(|data| !data.is_empty()) {
                self.pending_frame = Some((data, 0));
            }
            if frame.eof {
                self.recv_eof = true;
            }
        }
    }

    /// Whether the peer's eof frame has arrived and all earlier bytes have been read.
    pub fn recv_finished(&mut self) -> bool {
        self.fill();
        self.receive_error.is_none()
            && self.recv_eof
            && self.read_buf.is_empty()
            && self.pending_frame.is_none()
    }

    /// The sticky terminal receive error, if any. Calling this decodes ready frames
    /// up to the read buffer bound. A packet may have been proved at Channel admission
    /// before an earlier queued frame reaches this decoder; callers must treat this
    /// error as a failed link and must not present a healthy EOF.
    pub fn receive_error(&mut self) -> Option<StreamDecodeError> {
        self.fill();
        self.receive_error
    }

    /// Compatibility flag for callers using the previous diagnostic API.
    pub fn had_unsupported_frame(&self) -> bool {
        self.receive_error.is_some()
    }

    /// Envelopes to put on the wire now — see [`Channel::poll_transmit`].
    pub fn poll_transmit(&mut self, now: u64) -> Vec<Envelope> {
        self.channel.poll_transmit(now)
    }

    /// Feed a received envelope in — see [`Channel::handle`]. Returns whether the driver
    /// should prove the packet (`false` when the reorder buffer is full).
    #[must_use]
    pub fn handle(&mut self, envelope: Envelope) -> bool {
        if self.receive_error.is_some() {
            return false;
        }
        self.channel.handle(envelope)
    }

    /// Release a proven sequence — see [`Channel::on_proof`].
    pub fn on_proof(&mut self, sequence: u16, now: u64) {
        self.channel.on_proof(sequence, now);
    }

    /// The current send window — see [`Channel::window`].
    pub fn window(&self) -> u32 {
        self.channel.window()
    }

    /// The current RTT estimate — see [`Channel::rtt`].
    pub fn rtt(&self) -> u64 {
        self.channel.rtt()
    }

    /// Replace the initial RTT estimate — see [`Channel::set_initial_rtt`].
    pub fn set_initial_rtt(&mut self, rtt: u64) {
        self.channel.set_initial_rtt(rtt);
    }

    /// Set the per-envelope try limit — see [`Channel::set_max_tries`].
    pub fn set_max_tries(&mut self, tries: u8) {
        self.channel.set_max_tries(tries);
    }

    /// Why the underlying channel stopped, once it has — see [`Channel::error`].
    pub fn channel_error(&self) -> Option<ChannelError> {
        self.channel.error()
    }

    /// Whether everything written has been sent and proven.
    pub fn send_idle(&self) -> bool {
        self.channel.send_idle()
    }
}

#[cfg(test)]
mod tests {
    // The crate is `no_std`, so the tests take these from alloc rather than the std prelude.
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{
        Buffer, Channel, ChannelError, DEFAULT_MAX_TRIES, DEFAULT_RETX_TIMEOUT, Envelope,
        FAST_RATE_THRESHOLD, MAX_DATA_LEN, STREAM_ID_MAX, STREAM_MSGTYPE, StreamFrame,
        WINDOW_FLEXIBILITY, WINDOW_INITIAL, WINDOW_MAX, WINDOW_MAX_MEDIUM, WINDOW_MAX_SLOW,
        WINDOW_MIN_LIMIT_MEDIUM,
    };
    use crate::lossy::LossModel;

    #[test]
    fn envelope_matches_rns_capture() {
        // Gold test: retinue's envelope encoding equals RNS 1.3.8's own Envelope.pack()
        // for every captured vector. Ties the wire to the black-box capture.
        let fixture = include_str!("../tests/fixtures/channel_wire.json");
        let doc: serde_json::Value = serde_json::from_str(fixture).unwrap();
        for v in doc["envelope_vectors"].as_array().unwrap() {
            let msgtype = v["msgtype"].as_u64().unwrap() as u16;
            let sequence = v["sequence"].as_u64().unwrap() as u16;
            let payload = hex_bytes(v["payload_hex"].as_str().unwrap());
            let expected = v["packed_hex"].as_str().unwrap();
            let env = Envelope {
                msgtype,
                sequence,
                payload,
            };
            assert_eq!(
                hex_str(&env.encode()),
                expected,
                "encode must equal RNS pack()"
            );
            assert_eq!(Envelope::decode(&env.encode()), Some(env), "round-trip");
        }
    }

    fn hex_bytes(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    fn hex_str(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn lossless_in_order_delivery() {
        let mut tx: Channel = Channel::new(0xABCD);
        let mut rx: Channel = Channel::new(0xABCD);
        for i in 0u8..20 {
            tx.send(vec![i]).expect("the send queue has room");
        }
        let mut got = Vec::new();
        for now in 0..1000 {
            for e in tx.poll_transmit(now) {
                let seq = e.sequence;
                let _ = rx.handle(e);
                tx.on_proof(seq, now); // lossless: every packet is immediately proven
            }
            while let Some(m) = rx.recv() {
                got.push(m[0]);
            }
            if got.len() == 20 {
                break;
            }
        }
        assert_eq!(got, (0u8..20).collect::<Vec<_>>());
    }

    /// Run a byte stream through two Buffers across a deterministic lossy pipe on a
    /// virtual clock, proving each delivered envelope back (subject to loss) — the
    /// proof-based model. Returns nothing; asserts exact reconstruction.
    fn stream_over_loss(drop_per_mille: u32, max_delay_ticks: u64, seed: u64, max_tries: u8) {
        let payload: Vec<u8> = (0..4000u32)
            .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
            .collect();
        let mut tx: Buffer = Buffer::new();
        let mut rx: Buffer = Buffer::new();
        tx.set_max_tries(max_tries);
        assert_eq!(
            tx.write(&payload),
            payload.len(),
            "the send queue took every byte"
        );

        let mut fwd = LossModel::new(seed)
            .drop_per_mille(drop_per_mille)
            .max_delay_ms(max_delay_ticks);
        let mut bwd = LossModel::new(seed ^ 0xFFFF)
            .drop_per_mille(drop_per_mille)
            .max_delay_ms(max_delay_ticks);

        // In flight: (arrival_tick, item). Forward carries envelopes; back carries the
        // sequence of a proof (the link auto-proves every received packet).
        let mut to_rx: Vec<(u64, Envelope)> = Vec::new();
        let mut to_tx: Vec<(u64, u16)> = Vec::new();
        let mut got: Vec<u8> = Vec::new();

        for now in 0..1_000_000u64 {
            for e in tx.poll_transmit(now) {
                if !fwd.should_drop() {
                    to_rx.push((now + 1 + fwd.delay_ms(), e));
                }
            }
            // Deliver due envelopes; prove each one back (dup or not).
            let mut still = Vec::new();
            for (t, e) in core::mem::take(&mut to_rx) {
                if t <= now {
                    let seq = e.sequence;
                    let _ = rx.handle(e);
                    if !bwd.should_drop() {
                        to_tx.push((now + 1 + bwd.delay_ms(), seq));
                    }
                } else {
                    still.push((t, e));
                }
            }
            to_rx = still;
            to_tx.retain(|(t, seq)| {
                if *t <= now {
                    tx.on_proof(*seq, now);
                    false
                } else {
                    true
                }
            });
            got.extend(rx.read_available());
            if got.len() == payload.len() && tx.send_idle() {
                break;
            }
        }
        assert_eq!(got, payload, "stream must reconstruct exactly over loss");
        assert_eq!(tx.channel_error(), None, "the sender never gave up");
        assert!(tx.send_idle(), "and every envelope was proved");
    }

    /// At 30% loss each way one try fails half the time, so RNS's five tries lose about
    /// one envelope in thirty; this raises the limit to exercise retransmission at that loss.
    #[test]
    fn stream_survives_drop() {
        stream_over_loss(300, 0, 11, 64);
    }

    #[test]
    fn stream_survives_drop_reorder_and_delay() {
        stream_over_loss(250, 6, 99, DEFAULT_MAX_TRIES);
    }

    /// At 60% loss each way a round trip succeeds 16% of the time, so RNS's five tries
    /// would give up on most envelopes. This exercises reordering and retransmission at
    /// that loss, so it raises the try limit.
    #[test]
    fn heavy_loss_still_converges() {
        stream_over_loss(600, 3, 7, 64);
    }

    /// Count how many envelopes `channel` puts on the wire to deliver `messages` messages over
    /// a lossless pipe whose one-way delay is `rtt/2` ticks (so a data->proof round trip is
    /// `rtt` ticks). Each sequence's proof returns once, `rtt` ticks after its first send;
    /// retransmits issued before then are the waste this measures.
    fn transmissions_over_rtt(
        mut channel: Channel,
        rtt: u64,
        messages: usize,
    ) -> Result<usize, (usize, ChannelError)> {
        use alloc::collections::BTreeMap;
        use std::collections::HashSet;
        for m in 0..messages {
            channel
                .send(vec![m as u8])
                .expect("the send queue has room");
        }
        let mut proof_at: BTreeMap<u64, Vec<u16>> = BTreeMap::new();
        let mut scheduled: HashSet<u16> = HashSet::new();
        let mut total = 0usize;
        for now in 0..2_000_000u64 {
            if let Some(seqs) = proof_at.remove(&now) {
                for s in seqs {
                    channel.on_proof(s, now);
                }
            }
            for env in channel.poll_transmit(now) {
                total += 1;
                if scheduled.insert(env.sequence) {
                    proof_at.entry(now + rtt).or_default().push(env.sequence);
                }
            }
            if let Some(error) = channel.error() {
                return Err((total, error));
            }
            if channel.send_idle() {
                break;
            }
        }
        assert!(channel.send_idle(), "the transfer completed");
        Ok(total)
    }

    /// The adaptive retransmit timeout must not storm a slow medium. Over a 1000-tick round
    /// trip, the dynamic channel keys its timeout off the RTT estimate and sends close to one
    /// transmission per message. A fixed 4-tick timeout retransmits before any proof can
    /// return, and so spends all its tries and gives up on a link that was working.
    #[test]
    fn adaptive_timeout_does_not_storm_a_high_rtt_link() {
        let messages = 16;
        let rtt = 1000;

        let adaptive = transmissions_over_rtt(Channel::new(STREAM_MSGTYPE), rtt, messages)
            .expect("the adaptive channel completes");
        let fixed_tiny = transmissions_over_rtt(
            Channel::with_params(STREAM_MSGTYPE, 8, DEFAULT_RETX_TIMEOUT),
            rtt,
            messages,
        );

        // The adaptive channel sends roughly one frame per message (a small startup burst is
        // allowed while its RTT estimate settles).
        assert!(
            adaptive < messages * 3,
            "adaptive sent {adaptive} for {messages} messages (should be near {messages})"
        );
        // The fixed tiny timeout exhausts every try of its first window before a proof lands.
        let Err((sent, error)) = fixed_tiny else {
            panic!("a 4-tick timeout over a 1000-tick round trip must give up");
        };
        assert_eq!(error, ChannelError::RetriesExhausted { sequence: 0 });
        assert_eq!(sent, 8 * usize::from(DEFAULT_MAX_TRIES));
    }

    #[test]
    fn max_window_one_serializes_half_duplex_turns() {
        let mut channel: Channel =
            Channel::with_initial_rtt_and_max_window(STREAM_MSGTYPE, 5_000, 1);
        channel.send(vec![1]).expect("the send queue has room");
        channel.send(vec![2]).expect("the send queue has room");

        let first = channel.poll_transmit(0);
        assert_eq!(first.len(), 1);
        assert_eq!(channel.window(), 1);
        assert!(channel.poll_transmit(1).is_empty());

        channel.on_proof(first[0].sequence, 2);
        let second = channel.poll_transmit(2);
        assert_eq!(second.len(), 1);
        assert_eq!(channel.window(), 1);
    }

    #[test]
    fn sequence_wraps_past_the_16bit_modulus() {
        // Push more than 65536 messages so the sequence wraps, and confirm order holds
        // across the wrap. Small window keeps it quick.
        let mut tx: Channel = Channel::with_params(0x0001, 4, 2);
        let mut rx: Channel = Channel::with_params(0x0001, 4, 2);
        let total = 70_000u32; // > SEQ_MODULUS
        let mut sent = 0u32;
        let mut got = 0u32;
        for now in 0..5_000_000u64 {
            while sent < total && tx.in_flight() < 4 && tx.send_room() > 0 {
                tx.send(vec![(sent % 251) as u8])
                    .expect("the send queue has room");
                sent += 1;
            }
            for e in tx.poll_transmit(now) {
                let seq = e.sequence;
                let _ = rx.handle(e);
                tx.on_proof(seq, now);
            }
            while let Some(m) = rx.recv() {
                assert_eq!(m, vec![(got % 251) as u8], "in order across the wrap");
                got += 1;
            }
            if got == total {
                break;
            }
        }
        assert_eq!(got, total, "all delivered across the sequence wrap");
    }

    /// A proved frame must never strand in the reorder buffer (review finding, 2026-07-31).
    ///
    /// A frame buffered out of order is proved on arrival, so the sender never retransmits
    /// it. If the inbox fills while the drain runs, the next contiguous frame stays in
    /// `reorder` with `recv_next` pointing at it, and no future arrival carries `recv_next`
    /// to re-trigger the drain in `handle`. Before `recv` pumped, that was a permanent
    /// stall with the data sitting on the receiver.
    #[test]
    fn a_proved_frame_never_strands_when_the_inbox_fills_mid_drain() {
        let env = |seq: u16| Envelope {
            msgtype: 0x0001,
            sequence: seq,
            payload: vec![seq as u8],
        };
        // QUEUE = 2: the inbox holds two frames, so delivering seq 0 with seqs 1 and 2
        // already buffered fills it mid-drain and leaves seq 2 behind in reorder.
        let mut rx: Channel<8, 2, 8> = Channel::with_params(0x0001, 4, 2);
        assert!(rx.handle(env(1)), "future frame is buffered and proved");
        assert!(
            rx.handle(env(2)),
            "second future frame is buffered and proved"
        );
        assert!(rx.handle(env(0)), "the gap frame is delivered and proved");

        // The sender got proofs for all three, so nothing will ever be retransmitted.
        // Reading must still deliver every frame in order.
        assert_eq!(rx.recv().as_deref(), Some(&[0u8][..]));
        assert_eq!(rx.recv().as_deref(), Some(&[1u8][..]));
        assert_eq!(
            rx.recv().as_deref(),
            Some(&[2u8][..]),
            "the frame the full inbox left in reorder must surface once the app makes room"
        );
        assert_eq!(rx.recv(), None, "and nothing further is owed");
    }

    /// Prove every envelope one tick after it is sent until `proofs` have landed.
    fn prove_promptly<const W: usize, const Q: usize, const R: usize>(
        c: &mut Channel<W, Q, R>,
        now: &mut u64,
        proofs: u32,
    ) {
        let mut proven = 0u32;
        while proven < proofs {
            let envs = c.poll_transmit(*now);
            *now += 1;
            for e in envs {
                if proven < proofs {
                    c.on_proof(e.sequence, *now);
                    proven += 1;
                }
            }
        }
    }

    #[test]
    fn window_shrinks_by_one_per_timeout() {
        // Grow the window into the fast tier, then let eight fresh envelopes go unproved past
        // their timeout. RNS steps the window down by one per timed-out envelope, and
        // window_max with it while it stays more than WINDOW_FLEXIBILITY above window_min.
        let mut c: Channel<64, 4096> = Channel::new(0x0001);
        for i in 0..2000u16 {
            c.send(vec![i as u8]).expect("the send queue has room");
        }
        let mut now = 0u64;
        prove_promptly(&mut c, &mut now, 200);
        // Let whatever is still in flight be proved, so only the fresh eight time out.
        for seq in 0..=u16::MAX {
            c.on_proof(seq, now);
        }
        assert_eq!(
            c.window(),
            WINDOW_MAX,
            "grew to the fast-tier ceiling first"
        );
        assert_eq!((c.window_max, c.window_min), (WINDOW_MAX, 16));

        let fresh = c.poll_transmit(now);
        assert_eq!(fresh.len(), WINDOW_MAX as usize, "a full window went out");
        let deadline = c.outstanding.values().map(|o| o.deadline).min().unwrap();
        assert!(
            c.poll_transmit(deadline - 1).is_empty(),
            "nothing before the deadline"
        );
        let resent = c.poll_transmit(deadline);
        assert_eq!(
            resent.len(),
            WINDOW_MAX as usize,
            "every envelope timed out together"
        );
        // 48 timeouts against a floor of 16: the window stops at the floor, and window_max
        // stops WINDOW_FLEXIBILITY above it.
        assert_eq!(c.window(), 16);
        assert_eq!(c.window_max, 16 + WINDOW_FLEXIBILITY);
    }

    #[test]
    fn a_single_timeout_costs_one_window_step() {
        let mut c: Channel<64, 4096> = Channel::new(0x0001);
        for i in 0..2000u16 {
            c.send(vec![i as u8]).expect("the send queue has room");
        }
        let mut now = 0u64;
        prove_promptly(&mut c, &mut now, 200);
        for seq in 0..=u16::MAX {
            c.on_proof(seq, now);
        }
        let grown = c.window();
        let sent = c.poll_transmit(now);
        // Prove all but the first, so exactly one envelope times out.
        for e in &sent[1..] {
            c.on_proof(e.sequence, now + 1);
        }
        let window_after_proofs = c.window();
        assert_eq!(window_after_proofs, grown, "already at window_max");
        let deadline = c.outstanding.values().next().unwrap().deadline;
        let resent: Vec<u16> = c
            .poll_transmit(deadline)
            .iter()
            .map(|e| e.sequence)
            .filter(|&s| s == sent[0].sequence)
            .collect();
        assert_eq!(resent, vec![sent[0].sequence]);
        assert_eq!(c.window(), grown - 1, "one timeout, one step");
        assert_eq!(c.window_max, WINDOW_MAX - 1);
    }

    #[test]
    fn the_window_grows_by_one_per_proof_up_to_the_slow_ceiling() {
        // A fresh channel sits in the slow tier: window 2, ceiling 5. A slow RTT never
        // promotes, so proofs open the window one at a time and stop at 5.
        let mut c: Channel = Channel::with_initial_rtt(0x0001, 1_000);
        assert_eq!((c.window(), c.window_max, c.window_min), (2, 5, 2));
        for i in 0..64u16 {
            c.send(vec![i as u8]).expect("the send queue has room");
        }
        let mut seen = Vec::new();
        let mut now = 0u64;
        for _ in 0..20 {
            let envs = c.poll_transmit(now);
            now += 1_000;
            for e in envs {
                c.on_proof(e.sequence, now);
                seen.push(c.window());
            }
        }
        assert_eq!(&seen[..4], &[3, 4, 5, 5], "one step per proof, capped at 5");
        assert_eq!(c.window(), 5, "a slow link never leaves the slow tier");
    }

    #[test]
    fn ten_medium_rounds_promote_to_the_medium_tier() {
        // RTT 500 ticks sits between the fast and medium bounds.
        let mut c: Channel = Channel::with_initial_rtt(0x0001, 500);
        for i in 0..64u16 {
            c.send(vec![i as u8]).expect("the send queue has room");
        }
        let mut now = 0u64;
        let mut proofs = 0u32;
        while proofs < FAST_RATE_THRESHOLD - 1 {
            let envs = c.poll_transmit(now);
            now += 500;
            for e in envs {
                if proofs < FAST_RATE_THRESHOLD - 1 {
                    c.on_proof(e.sequence, now);
                    proofs += 1;
                }
            }
        }
        assert_eq!(c.window_max, WINDOW_MAX_SLOW, "nine rounds are not enough");
        let next = *c.outstanding.keys().next().unwrap();
        c.on_proof(next, now);
        assert_eq!(c.window_max, WINDOW_MAX_MEDIUM, "the tenth round promotes");
        assert_eq!(c.window_min, WINDOW_MIN_LIMIT_MEDIUM);
    }

    #[test]
    fn window_grows_on_sustained_clean_proofs() {
        // A dynamic channel starts at WINDOW_INITIAL. Prove a long run of packets cleanly and
        // promptly (one-tick round trip): the RTT estimate falls into the fast tier, ten fast
        // rounds promote window_max to WINDOW_MAX, and the window climbs to it.
        let mut c: Channel<64, 4096> = Channel::new(0x0001);
        assert_eq!(c.window(), WINDOW_INITIAL, "starts at the initial window");
        for i in 0..2000u16 {
            c.send(vec![i as u8]).expect("the send queue has room");
        }
        let mut now = 0u64;
        prove_promptly(&mut c, &mut now, 2000);
        assert_eq!(c.window(), WINDOW_MAX, "climbed to the fast-tier ceiling");
    }

    #[test]
    fn a_link_slower_than_the_slow_tier_pins_the_window_to_one() {
        let c: Channel = Channel::with_initial_rtt(0x0001, 1_451);
        assert_eq!((c.window(), c.window_max, c.window_min), (1, 1, 1));
        let c: Channel = Channel::with_initial_rtt(0x0001, 1_450);
        assert_eq!((c.window(), c.window_max, c.window_min), (2, 5, 2));
    }

    #[test]
    fn a_link_rtt_arriving_before_any_send_reselects_the_starting_window() {
        // A responder builds its channel on a guess before the initiator's RTT packet; RNS
        // builds its channel after, so a slow link still pins the window to one.
        let mut c: Channel = Channel::with_initial_rtt(0x0001, 750);
        c.set_initial_rtt(2_000);
        assert_eq!((c.window(), c.window_max, c.window_min), (1, 1, 1));
        c.set_initial_rtt(300);
        assert_eq!((c.window(), c.window_max, c.window_min), (2, 5, 2));

        // Once something is on the wire the window is live state and is left alone.
        let mut c: Channel = Channel::with_initial_rtt(0x0001, 750);
        c.send(vec![1]).expect("the send queue has room");
        let _ = c.poll_transmit(0);
        c.set_initial_rtt(2_000);
        assert_eq!(c.window(), 2);
        assert_eq!(c.rtt, 2_000);
    }

    #[test]
    fn retransmits_back_off_by_half_again_and_give_up_after_five_tries() {
        // One envelope, RTT 100, never proved. RNS's timeout for try n with one envelope in
        // flight is 1.5^(n-1) * max(2.5 * 100, 25) * (1 + 1.5) = 625 * 1.5^(n-1) ticks.
        let mut c: Channel = Channel::with_initial_rtt(0x0001, 100);
        c.send(vec![7]).expect("the send queue has room");
        let mut sent_at = Vec::new();
        let mut failed_at = None;
        for now in 0..20_000u64 {
            sent_at.extend(c.poll_transmit(now).iter().map(|_| now));
            if c.error().is_some() {
                failed_at = Some(now);
                break;
            }
        }
        // 625, 937, 1406, 2109, 3164 (floored).
        assert_eq!(sent_at, vec![0, 625, 1_562, 2_968, 5_077]);
        assert_eq!(failed_at, Some(8_241));
        assert_eq!(
            c.error(),
            Some(ChannelError::RetriesExhausted { sequence: 0 })
        );
        assert!(!c.send_idle(), "a failed channel is never idle");
        assert_eq!(c.send(vec![8]), Err(vec![8]), "and takes no more data");
        assert!(c.poll_transmit(100_000).is_empty(), "nor sends anything");
    }

    #[test]
    fn more_in_flight_stretches_every_deadline() {
        // RNS raises every pending timeout when another envelope joins the ring.
        let mut c: Channel = Channel::with_initial_rtt(0x0001, 100);
        c.send(vec![1]).expect("the send queue has room");
        let _ = c.poll_transmit(0);
        assert_eq!(c.outstanding[&0].deadline, 625, "250 * (1 + 1.5)");
        c.send(vec![2]).expect("the send queue has room");
        let _ = c.poll_transmit(10);
        assert_eq!(c.outstanding[&0].deadline, 875, "250 * (2 + 1.5)");
        assert_eq!(c.outstanding[&1].deadline, 10 + 875);
    }

    #[test]
    fn karns_rule_skips_rtt_samples_from_retransmits() {
        let mut c: Channel = Channel::with_initial_rtt(0x0001, 100);
        c.send(vec![1]).expect("the send queue has room");
        let _ = c.poll_transmit(0);
        let resent = c.poll_transmit(625);
        assert_eq!(resent.len(), 1, "retransmitted once");
        // The proof might answer either transmission, so it says nothing about the RTT.
        c.on_proof(0, 700);
        assert_eq!(c.rtt, 100, "no sample from a retransmitted envelope");

        c.set_initial_rtt(400);
        assert_eq!(c.rtt, 400, "a link measurement replaces the guess");
        c.send(vec![2]).expect("the send queue has room");
        let _ = c.poll_transmit(1_000);
        c.on_proof(1, 1_020);
        assert_eq!(
            c.rtt, 20,
            "a first transmission is sampled, replacing the guess"
        );
        c.send(vec![3]).expect("the send queue has room");
        let _ = c.poll_transmit(2_000);
        c.on_proof(2, 2_060);
        assert_eq!(c.rtt, (20 * 7 + 60) / 8, "and later samples are smoothed");
        c.set_initial_rtt(400);
        assert_eq!(c.rtt, (20 * 7 + 60) / 8, "a measured RTT is not overridden");
    }

    #[test]
    fn stream_frame_matches_rns_capture() {
        // Gold test: retinue's StreamFrame encoding equals RNS 1.3.8's own
        // StreamDataMessage.pack() for every captured vector, and our constants match.
        let fixture = include_str!("../tests/fixtures/buffer_wire.json");
        let doc: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let c = &doc["constants"];
        assert_eq!(c["MSGTYPE"].as_u64().unwrap() as u16, STREAM_MSGTYPE);
        assert_eq!(c["STREAM_ID_MAX"].as_u64().unwrap() as u16, STREAM_ID_MAX);
        assert_eq!(c["MAX_DATA_LEN"].as_u64().unwrap() as usize, MAX_DATA_LEN);
        for v in doc["frame_vectors"].as_array().unwrap() {
            let frame = StreamFrame {
                stream_id: v["stream_id"].as_u64().unwrap() as u16,
                eof: v["eof"].as_bool().unwrap(),
                compressed: v["compressed"].as_bool().unwrap(),
                data: hex_bytes(v["data_hex"].as_str().unwrap()),
            };
            let expected = v["packed_hex"].as_str().unwrap();
            assert_eq!(
                hex_str(&frame.encode()),
                expected,
                "encode must equal RNS pack()"
            );
            assert_eq!(
                StreamFrame::decode(&frame.encode()),
                Some(frame),
                "round-trip"
            );
        }
    }

    #[test]
    fn buffer_demuxes_by_stream_id_and_signals_eof() {
        // One channel carries two streams (RNS multiplexes above the sequence). A reader
        // bound to stream 5 delivers only stream 5's bytes in order, ignores stream 9,
        // and reports eof from stream 5 — not from stream 9's earlier eof.
        let mut r5: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), 8, 0, 5);
        let feed = |r: &mut Buffer, seq: u16, f: StreamFrame| {
            let _ = r.handle(Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: seq,
                payload: f.encode(),
            });
        };
        feed(
            &mut r5,
            0,
            StreamFrame {
                stream_id: 5,
                eof: false,
                compressed: false,
                data: vec![1, 2, 3],
            },
        );
        feed(
            &mut r5,
            1,
            StreamFrame {
                stream_id: 9,
                eof: false,
                compressed: false,
                data: vec![0xAA],
            },
        );
        feed(
            &mut r5,
            2,
            StreamFrame {
                stream_id: 5,
                eof: false,
                compressed: false,
                data: vec![4, 5],
            },
        );
        feed(
            &mut r5,
            3,
            StreamFrame {
                stream_id: 9,
                eof: true,
                compressed: false,
                data: vec![],
            },
        );
        assert!(!r5.recv_finished(), "stream 9's eof must not end stream 5");
        feed(
            &mut r5,
            4,
            StreamFrame {
                stream_id: 5,
                eof: true,
                compressed: false,
                data: vec![6],
            },
        );
        assert_eq!(
            r5.read_available(),
            vec![1, 2, 3, 4, 5, 6],
            "only stream 5, in order"
        );
        assert!(r5.recv_finished(), "stream 5's eof");
    }

    #[test]
    fn an_undecodable_compressed_frame_is_terminal_after_the_prefix() {
        // Bytes flagged compressed that are not valid bz2 cannot be recovered. They must be
        // surfaced, never spliced into the stream as if they were data.
        let mut r: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), 8, 0, 0);
        let feed = |r: &mut Buffer, seq: u16, f: StreamFrame| {
            let _ = r.handle(Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: seq,
                payload: f.encode(),
            });
        };
        feed(
            &mut r,
            0,
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: false,
                data: vec![1, 2],
            },
        );
        feed(
            &mut r,
            1,
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: true,
                data: vec![9, 9, 9],
            },
        );
        feed(
            &mut r,
            2,
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: false,
                data: vec![3],
            },
        );
        assert_eq!(
            r.read_available(),
            vec![1, 2],
            "only the prefix before the bad frame is delivered"
        );
        #[cfg(feature = "compression")]
        assert_eq!(
            r.receive_error(),
            Some(super::StreamDecodeError::InvalidCompression)
        );
        #[cfg(not(feature = "compression"))]
        assert_eq!(
            r.receive_error(),
            Some(super::StreamDecodeError::UnsupportedCompression)
        );
        assert!(!r.recv_finished(), "failure is not healthy EOF");
        assert!(r.read_available().is_empty(), "later bytes stay blocked");
    }

    #[test]
    fn plain_frame_larger_than_read_bound_is_delivered_in_order() {
        type SmallBuffer = Buffer<64, 256, 256, 8>;
        let mut r = SmallBuffer::new();
        let data: Vec<u8> = (0..20).collect();
        assert!(
            r.handle(Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: 0,
                payload: StreamFrame {
                    stream_id: 0,
                    eof: true,
                    compressed: false,
                    data: data.clone(),
                }
                .encode(),
            })
        );

        let mut got = Vec::new();
        for _ in 0..3 {
            let chunk = r.read_available();
            assert!(chunk.len() <= 8);
            got.extend(chunk);
        }
        assert_eq!(got, data);
        assert!(r.recv_finished());
    }

    /// A compressed frame carries data, and the reliable layer has already proven it to the
    /// peer by the time it reaches the buffer. Dropping it is silent loss with an
    /// acknowledgement on it: the sender retires bytes the application never sees. This is
    /// the regression guard for that.
    #[cfg(feature = "compression")]
    #[test]
    fn a_compressed_frame_is_recovered_in_order() {
        let mut r: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), 8, 0, 0);
        let feed = |r: &mut Buffer, seq: u16, f: StreamFrame| {
            let _ = r.handle(Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: seq,
                payload: f.encode(),
            });
        };
        // Repetitive on purpose: bz2 only shrinks compressible input, and RNS compresses
        // only when it wins, so this is the shape that actually arrives flagged.
        let middle: Vec<u8> = std::iter::repeat_n(b'z', 512).collect();

        feed(
            &mut r,
            0,
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: false,
                data: vec![1, 2],
            },
        );
        feed(
            &mut r,
            1,
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: true,
                data: crate::resource::compress(&middle),
            },
        );
        feed(
            &mut r,
            2,
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: false,
                data: vec![3],
            },
        );

        let mut expected = vec![1, 2];
        expected.extend_from_slice(&middle);
        expected.push(3);
        assert_eq!(r.read_available(), expected, "recovered, and in wire order");
        assert!(
            !r.had_unsupported_frame(),
            "a frame this build can decode is not unsupported",
        );
    }

    /// One bz2 frame can expand beyond the read queue's capacity. Keep later frames
    /// behind it and deliver every byte across repeated bounded reads.
    #[cfg(feature = "compression")]
    #[test]
    fn expanded_frame_respects_read_bound_without_losing_following_data() {
        type SmallBuffer = Buffer<64, 256, 256, 8>;
        let mut r = SmallBuffer::new();
        let middle = vec![b'z'; 40];
        for (sequence, frame) in [
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: true,
                data: crate::resource::compress(&middle),
            },
            StreamFrame {
                stream_id: 0,
                eof: true,
                compressed: false,
                data: vec![1, 2, 3],
            },
        ]
        .into_iter()
        .enumerate()
        {
            assert!(r.handle(Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: sequence as u16,
                payload: frame.encode(),
            }));
        }

        let mut got = Vec::new();
        let mut first = [0u8; 3];
        assert_eq!(r.read(&mut first), first.len());
        got.extend_from_slice(&first);
        assert!(r.read_buf.len() <= 8);
        assert!(
            !r.recv_finished(),
            "eof follows the pending compressed data"
        );

        for _ in 0..10 {
            let chunk = r.read_available();
            assert!(chunk.len() <= 8, "one read exceeded READ_BYTES");
            assert!(
                r.read_buf.len() <= 8,
                "the queued bytes exceeded READ_BYTES"
            );
            got.extend(chunk);
            if got.len() == middle.len() + 3 {
                break;
            }
        }
        let mut expected = middle;
        expected.extend_from_slice(&[1, 2, 3]);
        assert_eq!(
            got, expected,
            "the expanded frame and EOF frame stay in order"
        );
        assert!(r.recv_finished());
        assert!(!r.had_unsupported_frame());
    }

    #[cfg(feature = "compression")]
    #[test]
    fn oversized_compressed_frame_stops_before_eof_and_bounds_output_allocation() {
        type SmallBuffer = Buffer<64, 256, 256, 8>;
        let mut r = SmallBuffer::new();
        assert_eq!(
            r.set_decoded_frame_limit(0),
            Err(super::StreamDecodeLimitError::InvalidLimit)
        );
        assert_eq!(
            r.set_decoded_frame_limit(usize::MAX),
            Err(super::StreamDecodeLimitError::InvalidLimit)
        );
        r.set_decoded_frame_limit(32).unwrap();
        let expanded = vec![b'x'; 100_000];
        let compressed = crate::resource::compress(&expanded);
        assert!(
            compressed.len() < 200,
            "small wire frame expands far past the ceiling"
        );
        let err = crate::resource::decompress_bounded(&compressed, 32).unwrap_err();
        assert_eq!(err, crate::resource::BoundedDecompressError::LimitExceeded);
        let exact =
            crate::resource::decompress_bounded(&crate::resource::compress(&expanded[..32]), 32)
                .unwrap();
        assert_eq!(exact.len(), 32);
        assert_eq!(
            exact.capacity(),
            33,
            "owned output allocation includes one sentinel byte"
        );

        for (sequence, frame) in [
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: false,
                data: vec![1, 2],
            },
            StreamFrame {
                stream_id: 0,
                eof: true,
                compressed: true,
                data: compressed,
            },
            StreamFrame {
                stream_id: 0,
                eof: true,
                compressed: false,
                data: vec![3],
            },
        ]
        .into_iter()
        .enumerate()
        {
            assert!(r.handle(Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: sequence as u16,
                payload: frame.encode()
            }));
        }
        assert_eq!(r.read_available(), vec![1, 2]);
        assert_eq!(
            r.receive_error(),
            Some(super::StreamDecodeError::DecodedFrameLimitExceeded { limit: 32 })
        );
        assert!(!r.recv_finished());
        assert!(r.read_available().is_empty());
    }

    #[cfg(feature = "compression")]
    #[test]
    fn eof_on_expanded_frame_waits_for_all_bytes_to_be_read() {
        type SmallBuffer = Buffer<64, 256, 256, 8>;
        let mut r = SmallBuffer::new();
        let data = vec![b'x'; 20];
        assert!(
            r.handle(Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: 0,
                payload: StreamFrame {
                    stream_id: 0,
                    eof: true,
                    compressed: true,
                    data: crate::resource::compress(&data),
                }
                .encode(),
            })
        );

        assert!(!r.recv_finished(), "eof must wait behind buffered data");
        let mut got = Vec::new();
        for _ in 0..3 {
            let chunk = r.read_available();
            assert!(chunk.len() <= 8);
            got.extend(chunk);
            if got.len() < data.len() {
                assert!(!r.recv_finished(), "pending bytes must precede eof");
            }
        }
        assert_eq!(got, data);
        assert!(r.recv_finished(), "eof follows the last read byte");
    }

    #[test]
    fn buffer_stream_round_trips_with_finish() {
        // The everyday path: write a payload and finish() over the lossless proof model;
        // the reader reconstructs it exactly and sees eof.
        let mut tx: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), MAX_DATA_LEN, 3, 3);
        let mut rx: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), MAX_DATA_LEN, 3, 3);
        let payload: Vec<u8> = (0..2000u32).map(|i| (i * 7 + 1) as u8).collect();
        assert_eq!(
            tx.write(&payload),
            payload.len(),
            "the send queue took every byte"
        );
        assert!(tx.finish(), "the send queue had room for eof");
        let mut got = Vec::new();
        for now in 0..100_000u64 {
            let envs = tx.poll_transmit(now);
            if envs.is_empty() && tx.send_idle() {
                break;
            }
            for e in envs {
                let seq = e.sequence;
                let _ = rx.handle(e);
                tx.on_proof(seq, now);
            }
            got.extend(rx.read_available());
        }
        got.extend(rx.read_available());
        assert_eq!(got, payload, "stream reconstructs exactly");
        assert!(rx.recv_finished(), "reader saw the writer's eof");
    }

    /// A Buffer reads only stream messages (review #6). Another message type is
    /// sequenced and proved, so the stream moves past it, but its bytes are never read as
    /// a stream frame: an `80 00` payload would otherwise be an eof frame on stream 0, and
    /// `40 00` a compressed frame that fails to decode and tears the stream down.
    #[test]
    fn buffer_ignores_messages_that_are_not_stream_data() {
        let mut rx: Buffer = Buffer::new();
        let foreign = |sequence: u16, payload: &[u8]| Envelope {
            msgtype: 0x0101,
            sequence,
            payload: payload.to_vec(),
        };
        assert!(
            rx.handle(foreign(0, &[0x80, 0x00, b'x', b'y'])),
            "a foreign message is still proved"
        );
        assert!(rx.handle(foreign(1, &[0x40, 0x00, 0xde, 0xad])));
        assert!(rx.read_available().is_empty(), "no bytes reach the reader");
        assert!(!rx.recv_finished(), "a foreign message cannot set eof");
        assert_eq!(rx.receive_error(), None, "nor raise a receive error");

        // The sequence moved past both, so the next stream frame delivers in order.
        let frame = StreamFrame {
            stream_id: 0,
            eof: true,
            compressed: false,
            data: b"ok".to_vec(),
        };
        assert!(rx.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: 2,
            payload: frame.encode(),
        }));
        assert_eq!(rx.read_available(), b"ok".to_vec());
        assert!(rx.recv_finished(), "the stream's own eof still counts");
    }

    #[test]
    fn channel_reports_each_message_type() {
        let mut rx: Channel = Channel::new(STREAM_MSGTYPE);
        for (sequence, msgtype) in [(1u16, 0x0101u16), (0, STREAM_MSGTYPE)] {
            assert!(rx.handle(Envelope {
                msgtype,
                sequence,
                payload: vec![sequence as u8],
            }));
        }
        assert_eq!(rx.recv_message(), Some((STREAM_MSGTYPE, vec![0])));
        assert_eq!(rx.recv_message(), Some((0x0101, vec![1])));
        assert_eq!(rx.recv_message(), None);
    }
}
