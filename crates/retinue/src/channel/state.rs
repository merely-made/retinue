//! The [`Channel`] type: its state, construction, configuration, and accessors.

use alloc::vec::Vec;

use heapless::Deque;
use heapless::index_map::FnvIndexMap;

use super::STREAM_MSGTYPE;
use super::window::{
    DEFAULT_MAX_TRIES, RTT_MEDIUM, RTT_SLOW, WINDOW_FLEXIBILITY, WINDOW_INITIAL, WINDOW_MAX,
    WINDOW_MAX_SLOW, WINDOW_MIN,
};

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

/// The most out-of-order future envelopes the receiver holds at once. A well-behaved sender
/// keeps at most `WINDOW_MAX` (48) in flight; this bounds the buffer against a peer that
/// streams only future sequences and never fills the gap. See [`Channel::handle`].
pub const REORDER_MAX: usize = 256;

/// One un-acknowledged outbound envelope.
pub(super) struct Outstanding {
    pub(super) payload: Vec<u8>,
    /// When it last went on the wire.
    pub(super) last_tx: u64,
    /// When it is next due for a retransmit (or, out of tries, for giving up).
    pub(super) deadline: u64,
    /// How many times it has gone on the wire (RNS `Envelope.tries`).
    pub(super) tries: u8,
}

/// A reliable, in-order message channel. See the module docs.
///
/// The parameters bound its tables and default to the desktop profile; a board writes
/// [`SmallChannel`](crate::capacity::small_types::SmallChannel).
///
/// - `WINDOW` caps in-flight envelopes, and the send window is clamped to it at construction.
/// - `QUEUE` bounds the application-facing queues in each direction.
/// - `REORDER` bounds held-back future sequences, defaulting to [`REORDER_MAX`].
///
/// Payloads stay on the heap, so an entry costs a `Vec` header rather than
/// [`MAX_DATA_LEN`](super::MAX_DATA_LEN) bytes; what is bounded is the entry count, which is
/// what grows without limit on a lossy medium.
pub struct Channel<
    const WINDOW: usize = 64,
    const QUEUE: usize = 256,
    const REORDER: usize = REORDER_MAX,
> {
    pub(super) msgtype: u16,
    pub(super) window: u32,
    /// Current growth ceiling (RNS `window_max`): promoted by RTT tier, lowered by timeouts.
    pub(super) window_max: u32,
    /// Current shrink floor (RNS `window_min`).
    pub(super) window_min: u32,
    /// Smallest gap timeouts leave between `window_max` and `window_min`.
    pub(super) window_flexibility: u32,
    /// Caller-selected ceiling over every tier. A value of one serializes data/proof turns
    /// on strict half-duplex media.
    pub(super) max_window: u32,
    /// The fixed retransmit timeout of a [`with_params`](Self::with_params) channel.
    pub(super) retx_timeout: u64,
    /// Whether the window and timeout adapt. Off for a fixed window (`with_params`).
    pub(super) dynamic: bool,
    /// Consecutive proofs measured inside the fast / medium RTT tier.
    pub(super) fast_rate_rounds: u32,
    pub(super) medium_rate_rounds: u32,
    /// EWMA of the proof round-trip, in the caller's tick unit; selects the RTT tier.
    pub(super) rtt: u64,
    /// Whether `rtt` is measured yet, or still the caller's initial guess.
    pub(super) rtt_measured: bool,
    /// Transmissions per envelope before the channel fails.
    pub(super) max_tries: u8,
    /// Set once the channel has given up; see [`error`](Self::error).
    pub(super) error: Option<ChannelError>,

    // ── send side ──
    /// Application payloads not yet assigned a sequence (waiting for window room).
    pub(super) outgoing: Deque<Vec<u8>, QUEUE>,
    /// In-flight, unacknowledged, keyed by sequence. Released by [`on_proof`].
    pub(super) outstanding: FnvIndexMap<u16, Outstanding, WINDOW>,
    /// The next sequence to assign (wraps at `SEQ_MODULUS`).
    pub(super) send_next: u16,

    // ── receive side ──
    /// The next sequence we can deliver in order.
    pub(super) recv_next: u16,
    /// Received-but-not-yet-deliverable, held until the gap before them fills.
    ///
    /// Keyed rather than ordered: delivery pulls `recv_next` by exact key, so this never
    /// iterates in sequence order and does not need an ordered map.
    pub(super) reorder: FnvIndexMap<u16, (u16, Vec<u8>), REORDER>,
    /// Delivered, in order, ready for the application to read, each with its msgtype.
    pub(super) inbox: Deque<(u16, Vec<u8>), QUEUE>,
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

    /// Queue a payload for reliable, in-order delivery; [`poll_transmit`](Self::poll_transmit)
    /// sends it as the window allows. Returns the payload back when the send queue is full or
    /// the channel has failed, so a writer outpacing the link is told rather than queued forever.
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

    /// Whether everything queued to send has been sent and proven.
    pub fn send_idle(&self) -> bool {
        self.error.is_none() && self.outgoing.is_empty() && self.outstanding.is_empty()
    }

    /// Count of in-flight, unproven envelopes.
    pub fn in_flight(&self) -> usize {
        self.outstanding.len()
    }
}
