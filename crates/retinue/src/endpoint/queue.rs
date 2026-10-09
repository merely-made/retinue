//! Per-interface outbound scheduling: traffic classes and deficit round robin.

use alloc::vec::Vec;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::ifac::Ifac;
use crate::packet::Packet;

/// A per-class tally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassCounters {
    pub control: u64,
    pub interactive: u64,
    pub background: u64,
    pub transit: u64,
}

impl ClassCounters {
    pub(super) fn from_array(a: [u64; TrafficClass::COUNT]) -> Self {
        Self {
            control: a[TrafficClass::Control.index()],
            interactive: a[TrafficClass::Interactive.index()],
            background: a[TrafficClass::Background.index()],
            transit: a[TrafficClass::Transit.index()],
        }
    }

    pub(super) fn add(&mut self, other: Self) {
        self.control += other.control;
        self.interactive += other.interactive;
        self.background += other.background;
        self.transit += other.transit;
    }
}

/// What the outbound schedule has done, summed over every interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueCounters {
    /// Packets the schedule released to the wire, by class.
    pub sent: ClassCounters,
    /// Packets dropped because their class's queue was full, by class. Dropped transit on a
    /// healthy node means a neighbour offers more than this node agreed to carry.
    pub dropped: ClassCounters,
}

/// What a packet is for, which decides how it shares a busy interface. Classified at the send
/// site, since only the sender knows a chat message from a bulk sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrafficClass {
    /// Protocol upkeep: announces, path responses, link setup, proofs. Served first, since
    /// starving it stops the network repairing itself.
    Control,
    /// Local traffic someone is waiting on.
    Interactive,
    /// Local traffic that can wait: bulk transfer, replication.
    Background,
    /// Someone else's traffic, carried as a courtesy. Served last and capped.
    Transit,
}

impl TrafficClass {
    const COUNT: usize = 4;

    const fn index(self) -> usize {
        match self {
            Self::Control => 0,
            Self::Interactive => 1,
            Self::Background => 2,
            Self::Transit => 3,
        }
    }

    const ALL: [Self; Self::COUNT] = [
        Self::Control,
        Self::Interactive,
        Self::Background,
        Self::Transit,
    ];
}

/// Each class's share of a busy interface, as a deficit-round-robin quantum multiplier.
///
/// Relative shares, not rate limits: they bind only when more is offered than the medium
/// carries, which is when a host's own traffic must not lose to transit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueWeights {
    pub control: u32,
    pub interactive: u32,
    pub background: u32,
    pub transit: u32,
}

impl Default for QueueWeights {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl QueueWeights {
    /// Control first, then interactive, background, and transit last. The ratios matter more
    /// than the values: transit gets a share, never priority.
    pub const DEFAULT: Self = Self {
        control: 8,
        interactive: 4,
        background: 2,
        transit: 1,
    };

    fn for_class(&self, class: TrafficClass) -> u32 {
        match class {
            TrafficClass::Control => self.control,
            TrafficClass::Interactive => self.interactive,
            TrafficClass::Background => self.background,
            TrafficClass::Transit => self.transit,
        }
    }
}

/// How many packets each class may hold on one interface before its next packet is dropped.
/// An unbounded queue before a slow radio would turn memory into latency and hide the loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueDepths {
    pub control: usize,
    pub interactive: usize,
    pub background: usize,
    pub transit: usize,
}

impl Default for QueueDepths {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl QueueDepths {
    /// Transit is held shallowest: a neighbour's backlog is their problem to retry, not this
    /// node's to store.
    pub const DEFAULT: Self = Self {
        control: 64,
        interactive: 256,
        background: 256,
        transit: 64,
    };

    fn for_class(&self, class: TrafficClass) -> usize {
        match class {
            TrafficClass::Control => self.control,
            TrafficClass::Interactive => self.interactive,
            TrafficClass::Background => self.background,
            TrafficClass::Transit => self.transit,
        }
    }
}

/// The deficit-round-robin quantum, in bytes, so classes share *airtime* rather than packet
/// count: large frames cannot take more than their class's share.
const QUANTUM_UNIT: u64 = 128;

#[derive(Default)]
struct QueueState {
    queues: [VecDeque<Packet>; TrafficClass::COUNT],
    deficit: [u64; TrafficClass::COUNT],
    dropped: [u64; TrafficClass::COUNT],
    sent: [u64; TrafficClass::COUNT],
    cursor: usize,
    /// Whether the class at `cursor` has been credited its quantum for this visit. Crediting
    /// on every `pop` would let a standing backlog starve every class below it.
    credited: bool,
    /// Packets handed to the interface pump whose delivery has not completed yet.
    in_flight: usize,
    closed: bool,
}

/// One interface's outbound scheduler: bounded per-class queues plus the parking spot for
/// whoever is draining them.
pub(super) struct OutboundQueues {
    state: Mutex<QueueState>,
    ready: tokio::sync::Notify,
    weights: Mutex<(QueueWeights, QueueDepths)>,
}

impl OutboundQueues {
    pub(super) fn new(weights: QueueWeights, depths: QueueDepths) -> Self {
        Self {
            state: Mutex::new(QueueState::default()),
            ready: tokio::sync::Notify::new(),
            weights: Mutex::new((weights, depths)),
        }
    }

    /// Queue a packet in its class. Returns `false` if that class is full and the packet was
    /// dropped, which is reported rather than hidden.
    pub(super) fn push(&self, pkt: Packet, class: TrafficClass) -> bool {
        let depth = self.weights.lock().unwrap().1.for_class(class);
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return false;
        }
        let i = class.index();
        if state.queues[i].len() >= depth {
            state.dropped[i] += 1;
            return false;
        }
        state.queues[i].push_back(pkt);
        drop(state);
        self.ready.notify_one();
        true
    }

    /// Whether every class is empty and the interface pump has completed the packet it
    /// most recently took.
    pub(super) fn is_drained(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.in_flight == 0 && state.queues.iter().all(VecDeque::is_empty)
    }

    /// Take the next packet the schedule allows, or `None` if every queue is empty.
    pub(super) fn pop(&self) -> Option<Packet> {
        let weights = self.weights.lock().unwrap().0;
        let mut state = self.state.lock().unwrap();
        if state.queues.iter().all(VecDeque::is_empty) {
            return None;
        }
        // Deficit round robin: a class is credited once per *visit* and spends that credit
        // until its head packet costs more than it has banked. A class that empties forfeits
        // its credit, so it cannot bank capacity while idle.
        //
        // Progress: each cycle credits every non-empty class at least QUANTUM_UNIT, and a
        // packet costs at most the MTU, so a class becomes affordable within a few cycles.
        for _ in 0..(TrafficClass::COUNT * 8) {
            let i = state.cursor;
            let class = TrafficClass::ALL[i];
            if state.queues[i].is_empty() {
                state.deficit[i] = 0;
                state.credited = false;
                state.cursor = (i + 1) % TrafficClass::COUNT;
                continue;
            }
            if !state.credited {
                // Clamped to 1: a zero weight would credit nothing and spin forever.
                state.deficit[i] += u64::from(weights.for_class(class).max(1)) * QUANTUM_UNIT;
                state.credited = true;
            }
            let cost = state.queues[i]
                .front()
                .map_or(1, |p| p.encoded_len() as u64)
                .max(1);
            if state.deficit[i] >= cost {
                state.deficit[i] -= cost;
                let pkt = state.queues[i].pop_front();
                if state.queues[i].is_empty() {
                    state.deficit[i] = 0;
                    state.credited = false;
                    state.cursor = (i + 1) % TrafficClass::COUNT;
                }
                state.sent[i] += 1;
                state.in_flight += 1;
                return pkt;
            }
            state.credited = false;
            state.cursor = (i + 1) % TrafficClass::COUNT;
        }
        // Unreachable by the progress argument, but returning None with packets queued would
        // park the drain forever, so fall back to strict order.
        for i in 0..TrafficClass::COUNT {
            if let Some(pkt) = state.queues[i].pop_front() {
                state.sent[i] += 1;
                state.in_flight += 1;
                return Some(pkt);
            }
        }
        None
    }

    pub(super) fn delivery_complete(&self) {
        let mut state = self.state.lock().unwrap();
        debug_assert!(state.in_flight > 0);
        state.in_flight = state.in_flight.saturating_sub(1);
    }

    pub(super) fn set_policy(&self, weights: QueueWeights, depths: QueueDepths) {
        *self.weights.lock().unwrap() = (weights, depths);
    }

    pub(super) fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_waiters();
    }

    pub(super) fn counters(&self) -> ([u64; TrafficClass::COUNT], [u64; TrafficClass::COUNT]) {
        let state = self.state.lock().unwrap();
        (state.sent, state.dropped)
    }

    pub(super) fn depth(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.in_flight + state.queues.iter().map(VecDeque::len).sum::<usize>()
    }
}

/// The draining half of an interface's outbound path: `recv().await` yields packets in the
/// schedule's order.
pub struct OutboundPackets {
    pub(super) queues: Arc<OutboundQueues>,
    pub(super) delivery_in_flight: bool,
    pub(super) ifac: Option<Ifac>,
}

impl OutboundPackets {
    /// The next packet to put on the wire, or `None` once the endpoint is gone.
    pub async fn recv(&mut self) -> Option<Packet> {
        self.complete_delivery();
        loop {
            if let Some(pkt) = self.queues.pop() {
                self.delivery_in_flight = true;
                return Some(pkt);
            }
            if self.queues.state.lock().unwrap().closed {
                return None;
            }
            // Register before re-checking, so a push between the pop above and the wait here
            // cannot be missed.
            let notified = self.queues.ready.notified();
            if let Some(pkt) = self.queues.pop() {
                self.delivery_in_flight = true;
                return Some(pkt);
            }
            if self.queues.state.lock().unwrap().closed {
                return None;
            }
            notified.await;
        }
    }

    /// Encode one queued packet for this interface, applying IFAC when configured.
    pub fn encode(&self, packet: &Packet) -> crate::Result<Vec<u8>> {
        let logical = packet.encode();
        match &self.ifac {
            Some(ifac) => ifac.seal(&logical),
            None => Ok(logical),
        }
    }

    /// Drop everything queued, counted as dropped: an offline carrier sends nothing
    /// (`Transport.py` 1449), and a stale backlog must not flush on reconnect.
    pub(super) fn discard(&mut self) {
        self.complete_delivery();
        let mut state = self.queues.state.lock().unwrap();
        let state = &mut *state;
        for (queue, dropped) in state.queues.iter_mut().zip(&mut state.dropped) {
            *dropped += queue.len() as u64;
            queue.clear();
        }
    }

    fn complete_delivery(&mut self) {
        if self.delivery_in_flight {
            self.queues.delivery_complete();
            self.delivery_in_flight = false;
        }
    }
}

impl Drop for OutboundPackets {
    fn drop(&mut self) {
        self.complete_delivery();
    }
}
