//! A propagation node's state and policy: the store, who may fetch, which stamps are
//! admitted, and who is throttled.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rmpv::Value;

use super::msgpack::GetRequest;
use super::{
    PropagationAnnounce, PropagationCosts, PropagationEntry, PropagationStore, ServedFetch,
};

/// How long an identified sender of a bad stamp is refused (`LXMRouter.py` 67, 2515-2523).
pub const STAMP_THROTTLE: Duration = Duration::from_secs(180);
/// How long a node keeps a silent client link (`LXMRouter.py` 40, 978).
pub const LINK_MAX_INACTIVITY: Duration = Duration::from_secs(3 * 60);

/// What a node admits and whom it serves.
#[derive(Clone, Debug)]
pub struct NodePolicy {
    /// The announced costs. A stamp worth at least `propagation - flexibility` is
    /// admitted (`LXMRouter.py` 2311, 2483).
    pub costs: PropagationCosts,
    /// The largest submission, refused at its advertisement before any transfer
    /// (`LXMRouter.py` 2289-2292).
    pub max_transfer_bytes: usize,
    /// When set, only these identity hashes may fetch; others get 0xf1
    /// (`LXMRouter.py` 465-484, 1480-1492).
    pub allowed: Option<HashSet<[u8; 16]>>,
    /// How long a client link may stay silent before the node drops it.
    pub link_idle: Duration,
}

impl NodePolicy {
    /// The policy a node announcing `announce` enforces: its costs, and its sync limit
    /// as the transfer ceiling, as stock checks a submission against what it announced.
    pub fn from_announce(announce: &PropagationAnnounce) -> Self {
        Self {
            costs: announce.costs.clone(),
            max_transfer_bytes: usize::try_from(announce.sync_limit_kib.saturating_mul(1_000))
                .unwrap_or(usize::MAX),
            allowed: None,
            link_idle: LINK_MAX_INACTIVITY,
        }
    }

    pub fn stamp_floor(&self) -> u16 {
        u16::from(
            self.costs
                .propagation
                .saturating_sub(self.costs.flexibility),
        )
    }

    pub fn allows(&self, identity_hash: &[u8; 16]) -> bool {
        self.allowed
            .as_ref()
            .is_none_or(|allowed| allowed.contains(identity_hash))
    }
}

/// A propagation node shared by its link tasks. Locks are held only between awaits.
#[derive(Debug)]
pub struct PropagationNode {
    policy: NodePolicy,
    store: Mutex<PropagationStore>,
    throttled: Mutex<HashMap<[u8; 16], f64>>,
}

impl PropagationNode {
    pub fn new(store: PropagationStore, policy: NodePolicy) -> Self {
        Self {
            policy,
            store: Mutex::new(store),
            throttled: Mutex::new(HashMap::new()),
        }
    }

    pub fn policy(&self) -> &NodePolicy {
        &self.policy
    }

    /// The store, for snapshots and inspection.
    pub fn store(&self) -> MutexGuard<'_, PropagationStore> {
        self.store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn into_store(self) -> PropagationStore {
        self.store
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether `identity_hash` sent a bad stamp within [`STAMP_THROTTLE`].
    pub fn is_throttled(&self, identity_hash: &[u8; 16], now: f64) -> bool {
        let mut throttled = self.throttled.lock().unwrap_or_else(|p| p.into_inner());
        throttled.retain(|_, until| *until > now);
        throttled.contains_key(identity_hash)
    }

    pub(super) fn throttle(&self, identity_hash: [u8; 16], now: f64) {
        let mut throttled = self.throttled.lock().unwrap_or_else(|p| p.into_inner());
        throttled.retain(|_, until| *until > now);
        throttled.insert(identity_hash, now + STAMP_THROTTLE.as_secs_f64());
    }

    /// Answer one `/get` for `destination`, recording what was offered, acknowledged and
    /// served. An acknowledgement-only request answers `[]`.
    pub(super) fn answer_get(
        &self,
        destination: [u8; 16],
        request: GetRequest,
        now: f64,
        report: &mut ServedFetch,
    ) -> Value {
        let mut store = self.store();
        store.prune(now);
        let ids = |ids: &[[u8; 32]]| ids.iter().map(|id| Value::Binary(id.to_vec())).collect();
        match request {
            GetRequest::Offer => {
                report.offered = store.offer(destination);
                Value::Array(ids(&report.offered))
            }
            GetRequest::Fetch {
                wanted,
                handled,
                limit_kb,
            } => {
                report.acknowledged += store.acknowledge(destination, &handled);
                let messages = store.select(destination, &wanted, limit_kb.map(|kb| kb * 1_000.0));
                report
                    .served
                    .extend(messages.iter().map(|message| message.transient_id()));
                Value::Array(
                    messages
                        .iter()
                        .map(|message| Value::Binary(message.encode()))
                        .collect(),
                )
            }
        }
    }
}

/// Score each entry's stamp and keep those at or above `floor`, with their values.
/// Seconds of CPU for a large batch, so hosts run it off the executor.
pub(super) fn score_stamps(
    entries: Vec<PropagationEntry>,
    floor: u16,
) -> (Vec<(PropagationEntry, u16)>, usize) {
    let total = entries.len();
    let valid: Vec<_> = entries
        .into_iter()
        .filter_map(|entry| {
            let value = entry.stamp_value();
            (value >= floor).then_some((entry, value))
        })
        .collect();
    let invalid = total - valid.len();
    (valid, invalid)
}
