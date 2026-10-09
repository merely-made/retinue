//! The bounded, caller-persisted propagation store.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Duration;

use super::{
    DEFAULT_MAX_STORED_MESSAGE_BYTES, PropagationBatch, PropagationEntry, PropagationMessage,
};
use crate::stamp::STAMP_LEN;

/// How long a processed transient id is remembered: six message lifetimes, 180 days
/// (`LXMRouter.py` 1013-1033).
pub const PROCESSED_TRANSIENT_ID_TTL: Duration = Duration::from_secs(180 * 24 * 60 * 60);
/// The most processed transient ids a store remembers; the oldest go first.
pub const MAX_PROCESSED_TRANSIENT_IDS: usize = 65_536;

/// Stock's response packing estimate: a container overhead, then a per-message one
/// (`LXMRouter.py` 1542-1543).
const RESPONSE_OVERHEAD: f64 = 24.0;
const PER_MESSAGE_OVERHEAD: f64 = 16.0;

#[derive(Clone, Debug)]
pub struct PropagationStoreLimits {
    pub max_entries: usize,
    /// Total stored bytes, stamps included.
    pub max_bytes: usize,
    /// The largest message the store admits, stamp excluded.
    pub max_message_bytes: usize,
    pub max_age: Duration,
    /// The most ids one offer lists, and the most messages one response carries.
    pub max_per_fetch: usize,
}

impl Default for PropagationStoreLimits {
    fn default() -> Self {
        Self {
            max_entries: 4_096,
            max_bytes: 8 * 1024 * 1024,
            // Keeps the default store conservative. Callers may raise this;
            // large fetch responses then use a request-bound Resource.
            max_message_bytes: DEFAULT_MAX_STORED_MESSAGE_BYTES,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_per_fetch: 256,
        }
    }
}

impl PropagationStoreLimits {
    /// The largest packed single-entry submission (`[timebase, [entry]]`, the timebase a
    /// float) whose entry this store admits. A node refuses a larger transfer at its
    /// advertisement, so it never proves a message the store would drop.
    pub fn max_submission_bytes(&self) -> usize {
        let message = self
            .max_message_bytes
            .min(self.max_bytes.saturating_sub(STAMP_LEN));
        let entry = message.saturating_add(STAMP_LEN);
        let bin_header = match entry {
            0..=0xff => 2,
            0x100..=0xffff => 3,
            _ => 5,
        };
        // fixarray(2), float64, fixarray(1), then the entry.
        entry.saturating_add(1 + 9 + 1 + bin_header)
    }

    /// [`max_submission_bytes`](Self::max_submission_bytes) in the announce's kilobytes,
    /// rounded up: the transfer and sync limits a node with this store announces.
    pub fn announced_limit_kb(&self) -> u64 {
        self.max_submission_bytes().div_ceil(1_000) as u64
    }
}

#[derive(Clone, Debug)]
pub(super) struct StoredPropagation {
    pub(super) message: PropagationMessage,
    /// `None` only for entries restored from a version 1 snapshot.
    pub(super) stamp: Option<[u8; STAMP_LEN]>,
    pub(super) stamp_value: u16,
    pub(super) received_at: f64,
    /// Stored size, stamp included, as stock sizes its message files.
    bytes: usize,
    /// Arrival order, which breaks eviction ties and orders snapshots.
    pub(super) seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreRestoreReceipt {
    pub loaded: usize,
    pub duplicates: usize,
    pub rejected_too_large: usize,
    pub expired: usize,
    pub evicted: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreReceipt {
    pub inserted: usize,
    /// Already stored, or processed within [`PROCESSED_TRANSIENT_ID_TTL`].
    pub duplicates: usize,
    pub rejected_too_large: usize,
    pub evicted: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StoreInsert {
    Inserted { evicted: usize },
    Duplicate,
    TooLarge,
}

/// Transient ids already handled, so a resubmission is not stored again after its
/// recipient fetched it (`LXMRouter.py` 2565-2568).
#[derive(Clone, Debug, Default)]
pub(super) struct ProcessedIds {
    pub(super) at: HashMap<[u8; 32], f64>,
    pub(super) order: VecDeque<[u8; 32]>,
}

impl ProcessedIds {
    pub(super) fn insert(&mut self, id: [u8; 32], at: f64) {
        if self.at.insert(id, at).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > MAX_PROCESSED_TRANSIENT_IDS {
            if let Some(oldest) = self.order.pop_front() {
                self.at.remove(&oldest);
            }
        }
    }

    /// Forget ids past the TTL from the front of the arrival order.
    fn prune(&mut self, now: f64) {
        let ttl = PROCESSED_TRANSIENT_ID_TTL.as_secs_f64();
        while let Some(oldest) = self.order.front() {
            if self.at.get(oldest).is_some_and(|seen| now - seen <= ttl) {
                break;
            }
            self.at.remove(oldest);
            self.order.pop_front();
        }
    }
}

/// Bounded propagation store with caller-persisted state.
///
/// This type owns admission, duplicate suppression, expiry, capacity eviction,
/// and owner-scoped offers. The host owns storage and durability policy: call
/// [`Self::encode_snapshot`] after mutations and restore those bytes with
/// [`Self::restore`]. Restoration re-derives transient ids and byte counts and
/// re-applies the current limits instead of trusting persisted indexes.
///
/// Over capacity, the heaviest entries go first: size times age in four-day units
/// (at least one), a tenth of that for a prioritised destination (`LXMRouter.py`
/// 1064-1075, 1191-1226).
#[derive(Clone, Debug)]
pub struct PropagationStore {
    pub(super) limits: PropagationStoreLimits,
    pub(super) entries: HashMap<[u8; 32], StoredPropagation>,
    by_destination: HashMap<[u8; 16], HashSet<[u8; 32]>>,
    /// Entries by arrival time, oldest first, so expiry pops only what has expired.
    by_age: BTreeMap<(u64, u64), [u8; 32]>,
    pub(super) processed: ProcessedIds,
    prioritised: HashSet<[u8; 16]>,
    bytes: usize,
    next_seq: u64,
}

impl PropagationStore {
    pub fn new(limits: PropagationStoreLimits) -> Self {
        Self {
            limits,
            entries: HashMap::new(),
            by_destination: HashMap::new(),
            by_age: BTreeMap::new(),
            processed: ProcessedIds::default(),
            prioritised: HashSet::new(),
            bytes: 0,
            next_seq: 0,
        }
    }

    pub fn limits(&self) -> &PropagationStoreLimits {
        &self.limits
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Keep this destination's messages longest under capacity pressure.
    pub fn prioritise(&mut self, destination: [u8; 16]) {
        self.prioritised.insert(destination);
    }

    /// Whether this transient id was stored or processed within the memory window.
    pub fn has_processed(&self, transient_id: &[u8; 32]) -> bool {
        self.processed.at.contains_key(transient_id) || self.entries.contains_key(transient_id)
    }

    /// The stamp value an entry arrived with; 0 for an entry kept without its stamp.
    pub fn stamp_value(&self, transient_id: &[u8; 32]) -> Option<u16> {
        self.entries
            .get(transient_id)
            .map(|entry| entry.stamp_value)
    }

    /// The stamped entry as it was submitted, for forwarding to a peer.
    pub fn stamped_entry(&self, transient_id: &[u8; 32]) -> Option<PropagationEntry> {
        let entry = self.entries.get(transient_id)?;
        Some(PropagationEntry {
            message: entry.message.clone(),
            stamp: entry.stamp?,
        })
    }

    /// Store a trusted batch, scoring each stamp. A node takes submissions through
    /// [`PropagationNode`](super::PropagationNode), which checks stamps first.
    pub fn ingest(&mut self, batch: &PropagationBatch, now: f64) -> StoreReceipt {
        let scored = batch
            .entries
            .iter()
            .map(|entry| {
                let value = entry.stamp_value();
                (entry.clone(), value)
            })
            .collect();
        self.ingest_scored(scored, now)
    }

    pub(super) fn ingest_scored(
        &mut self,
        entries: Vec<(PropagationEntry, u16)>,
        now: f64,
    ) -> StoreReceipt {
        self.prune(now);
        let mut receipt = StoreReceipt::default();
        for (entry, value) in entries {
            let id = entry.transient_id();
            if self.has_processed(&id) {
                receipt.duplicates += 1;
                continue;
            }
            match self.insert(entry.message, Some(entry.stamp), value, now, now) {
                StoreInsert::Inserted { evicted } => {
                    // Only a stored message is processed; a refused one may come again.
                    self.processed.insert(id, now);
                    receipt.inserted += 1;
                    receipt.evicted += evicted;
                }
                StoreInsert::Duplicate => receipt.duplicates += 1,
                StoreInsert::TooLarge => receipt.rejected_too_large += 1,
            }
        }
        receipt
    }

    /// Drop expired entries and expired processed ids. Returns the entries dropped.
    /// Costs only what has expired, so a node may prune on every request.
    pub fn prune(&mut self, now: f64) -> usize {
        let max_age = self.limits.max_age.as_secs_f64();
        let mut expired = 0;
        while let Some((_, id)) = self.by_age.first_key_value() {
            let id = *id;
            if now - self.entries[&id].received_at <= max_age {
                break;
            }
            self.remove(&id);
            expired += 1;
        }
        self.processed.prune(now);
        expired
    }

    /// Delete `handled` ids held for `destination`.
    pub(super) fn acknowledge(&mut self, destination: [u8; 16], handled: &[[u8; 32]]) -> usize {
        handled
            .iter()
            .filter(|id| self.holds(destination, id) && self.remove(id))
            .count()
    }

    /// The ids held for `destination`, smallest first (`LXMRouter.py` 1499-1511).
    pub(super) fn offer(&self, destination: [u8; 16]) -> Vec<[u8; 32]> {
        let Some(ids) = self.by_destination.get(&destination) else {
            return Vec::new();
        };
        let mut sized: Vec<(usize, u64, [u8; 32])> = ids
            .iter()
            .map(|id| (self.entries[id].bytes, self.entries[id].seq, *id))
            .collect();
        sized.sort_unstable();
        sized
            .into_iter()
            .take(self.limits.max_per_fetch)
            .map(|(_, _, id)| id)
            .collect()
    }

    /// The wanted messages for `destination`, in request order, within a byte budget
    /// counted as stock packs a response (`LXMRouter.py` 1530-1560). An entry that does
    /// not fit is skipped and a smaller one after it may still go.
    pub(super) fn select(
        &self,
        destination: [u8; 16],
        wanted: &[[u8; 32]],
        budget: Option<f64>,
    ) -> Vec<PropagationMessage> {
        let mut packed = RESPONSE_OVERHEAD;
        let mut selected = Vec::new();
        for id in wanted {
            if selected.len() == self.limits.max_per_fetch {
                break;
            }
            if !self.holds(destination, id) {
                continue;
            }
            let entry = &self.entries[id];
            let next = packed + entry.bytes as f64 + PER_MESSAGE_OVERHEAD;
            if budget.is_some_and(|budget| next > budget) {
                continue;
            }
            packed = next;
            selected.push(entry.message.clone());
        }
        selected
    }

    fn holds(&self, destination: [u8; 16], id: &[u8; 32]) -> bool {
        self.entries
            .get(id)
            .is_some_and(|entry| entry.message.destination == destination)
    }

    pub(super) fn insert(
        &mut self,
        message: PropagationMessage,
        stamp: Option<[u8; STAMP_LEN]>,
        stamp_value: u16,
        received_at: f64,
        now: f64,
    ) -> StoreInsert {
        match self.insert_unbounded(message, stamp, stamp_value, received_at) {
            StoreInsert::Inserted { .. } => StoreInsert::Inserted {
                evicted: self.evict(now),
            },
            refused => refused,
        }
    }

    /// Insert without evicting; restore inserts everything, then evicts once.
    pub(super) fn insert_unbounded(
        &mut self,
        message: PropagationMessage,
        stamp: Option<[u8; STAMP_LEN]>,
        stamp_value: u16,
        received_at: f64,
    ) -> StoreInsert {
        let id = message.transient_id();
        if self.entries.contains_key(&id) {
            return StoreInsert::Duplicate;
        }
        let message_bytes = 16 + message.encrypted.len();
        let bytes = message_bytes + stamp.map_or(0, |_| STAMP_LEN);
        if message_bytes > self.limits.max_message_bytes || bytes > self.limits.max_bytes {
            return StoreInsert::TooLarge;
        }
        self.by_destination
            .entry(message.destination)
            .or_default()
            .insert(id);
        self.by_age
            .insert((age_key(received_at), self.next_seq), id);
        self.entries.insert(
            id,
            StoredPropagation {
                message,
                stamp,
                stamp_value,
                received_at,
                bytes,
                seq: self.next_seq,
            },
        );
        self.next_seq += 1;
        self.bytes += bytes;
        StoreInsert::Inserted { evicted: 0 }
    }

    fn over_capacity(&self) -> bool {
        self.entries.len() > self.limits.max_entries || self.bytes > self.limits.max_bytes
    }

    /// Drop the heaviest entries until within capacity, ranking them once.
    pub(super) fn evict(&mut self, now: f64) -> usize {
        if !self.over_capacity() {
            return 0;
        }
        let mut ranked: Vec<(f64, u64, [u8; 32])> = self
            .entries
            .iter()
            .map(|(id, entry)| (self.weight(entry, now), entry.seq, *id))
            .collect();
        ranked.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut evicted = 0;
        for (_, _, id) in ranked {
            if !self.over_capacity() {
                break;
            }
            self.remove(&id);
            evicted += 1;
        }
        evicted
    }

    fn weight(&self, entry: &StoredPropagation, now: f64) -> f64 {
        let age_weight = ((now - entry.received_at) / (4.0 * 24.0 * 60.0 * 60.0)).max(1.0);
        let priority = if self.prioritised.contains(&entry.message.destination) {
            0.1
        } else {
            1.0
        };
        priority * age_weight * entry.bytes as f64
    }

    fn remove(&mut self, id: &[u8; 32]) -> bool {
        let Some(entry) = self.entries.remove(id) else {
            return false;
        };
        self.bytes -= entry.bytes;
        self.by_age.remove(&(age_key(entry.received_at), entry.seq));
        let destination = entry.message.destination;
        if let Some(ids) = self.by_destination.get_mut(&destination) {
            ids.remove(id);
            if ids.is_empty() {
                self.by_destination.remove(&destination);
            }
        }
        true
    }
}

/// A time as a key that sorts as the time does (`f64::total_cmp`'s order).
fn age_key(at: f64) -> u64 {
    let bits = at.to_bits();
    if bits >> 63 == 1 {
        !bits
    } else {
        bits | 1 << 63
    }
}
