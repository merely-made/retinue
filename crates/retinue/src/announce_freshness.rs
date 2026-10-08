//! Bounded, per-destination announce freshness, tied to the route it admitted.
//!
//! Adapted from RNS 1.5.7 announce acceptance (`Transport.py` 2207-2296, 2340-2349). A
//! path-table row carries the random blobs of the announces that set it. While the row
//! lives, a later announce for its destination is admitted only when its emission timebase
//! is strictly newer than the newest retained blob's. When the row goes, on expiry or a lost
//! interface (`Transport.py` 957-978, 1086-1090), its blobs go with it: the next announce is
//! a first sighting, including the same blob served from a transport's announce cache in
//! answer to a path request (`Transport.py` 3459-3530).
//!
//! The caller owns the route table, so it tells [`AnnounceFreshness::evaluate`] whether a
//! live route exists. A row whose route has gone is ignored and is reset when its next
//! announce is recorded. Rows are bounded by count; when the ledger evicts a row, the caller
//! drops that destination's route, so a live route always has its blobs.
//!
//! Callers first [`AnnounceFreshness::evaluate`] a verified announce, perform any fallible
//! admission that must leave no trace on refusal, and then
//! [`AnnounceFreshness::record_accepted`] it before route mutation, publication, or relay
//! scheduling. The ledger keeps no clock, so the module stays `no_std + alloc`.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::announce::AnnounceBlob;
use crate::hash::AddressHash;

/// Bounds for [`AnnounceFreshness`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessConfig {
    /// Maximum number of destinations with retained freshness state.
    pub destination_capacity: usize,
    /// Maximum retained announce blobs for each destination.
    ///
    /// The blobs only tell a replay from a stale announce. Admission compares emission
    /// timebases against the newest accepted one, which is kept separately, so trimming
    /// history never admits an old announce.
    pub blob_capacity: usize,
}

/// A freshness table cannot retain state with either capacity set to zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceFreshnessConfigError {
    /// The table would have no destination slots.
    ZeroDestinationCapacity,
    /// A destination would have no blob slots.
    ZeroBlobCapacity,
}

impl core::fmt::Display for AnnounceFreshnessConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ZeroDestinationCapacity => {
                f.write_str("announce freshness destination capacity is zero")
            }
            Self::ZeroBlobCapacity => f.write_str("announce freshness blob capacity is zero"),
        }
    }
}

impl core::error::Error for AnnounceFreshnessConfigError {}

impl AnnounceFreshnessConfig {
    fn validate(self) -> Result<Self, AnnounceFreshnessConfigError> {
        if self.destination_capacity == 0 {
            Err(AnnounceFreshnessConfigError::ZeroDestinationCapacity)
        } else if self.blob_capacity == 0 {
            Err(AnnounceFreshnessConfigError::ZeroBlobCapacity)
        } else {
            Ok(self)
        }
    }
}

/// The packet-derived fields that participate in receive freshness.
///
/// Hops, context, interface and transport id are deliberately absent: RNS 1.5.7 admits a
/// live route's announce on emission time alone once its gravity and unresponsive-path
/// carve-outs, which retinue does not model, are set aside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessCandidate {
    /// Announced destination.
    pub destination: AddressHash,
    /// Full nonce-and-timebase blob from the signed announce payload.
    pub blob: AnnounceBlob,
}

impl AnnounceFreshnessCandidate {
    /// The candidate's 40-bit emission timebase, decoded from [`Self::blob`].
    pub const fn timebase(self) -> u64 {
        self.blob.timebase()
    }
}

/// Why a candidate was admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceFreshnessAccept {
    /// No live route exists for this destination, so nothing is compared. Recording it
    /// starts a fresh blob history, as RNS does when it re-creates a culled path row.
    FirstSighting,
    /// A live route exists and this announce was emitted strictly after every blob it holds.
    NewerTimebase,
}

/// Why a candidate was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceFreshnessReject {
    /// The exact blob is in the live route's retained history.
    Replay,
    /// A different blob, emitted no later than the live route's newest.
    StaleTimebase,
}

/// The result of evaluating one candidate without modifying the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceFreshnessDecision {
    /// The caller may perform its announce effects and later record the candidate.
    Accept(AnnounceFreshnessAccept),
    /// The caller must leave announce-derived state unchanged.
    Reject(AnnounceFreshnessReject),
}

impl AnnounceFreshnessDecision {
    /// Whether this decision admits the candidate.
    pub const fn is_accepted(self) -> bool {
        matches!(self, Self::Accept(_))
    }
}

/// What recording one already-admitted candidate changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessRecord {
    /// An older retained blob removed because this destination reached its bound.
    pub evicted_blob: Option<AnnounceBlob>,
    /// A destination evicted to make room for a new one. The caller must drop its route.
    pub evicted_destination: Option<AddressHash>,
}

/// What changing a table's configured bounds removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessReconfigure {
    /// Historical blobs removed because the new blob bound was smaller.
    pub evicted_blobs: usize,
    /// Destinations removed by the new destination bound, oldest acceptance first. The
    /// caller must drop their routes.
    pub evicted_destinations: Vec<AddressHash>,
}

#[derive(Clone, Debug)]
struct Row {
    /// The newest emission timebase this route has accepted: RNS's
    /// `timebase_from_random_blobs` over the row, kept even when its blob is trimmed.
    timebase: u64,
    /// Accepted blobs, oldest first.
    blobs: VecDeque<AnnounceBlob>,
    /// This row's key in [`AnnounceFreshness::by_age`].
    order: u64,
}

/// A bounded, allocation-backed announce freshness table.
#[derive(Clone, Debug)]
pub struct AnnounceFreshness {
    config: AnnounceFreshnessConfig,
    rows: BTreeMap<AddressHash, Row>,
    // Acceptance order, rather than a caller tick, makes row eviction deterministic and
    // O(log n) without a clock.
    by_age: BTreeMap<u64, AddressHash>,
    next_order: u64,
}

impl AnnounceFreshness {
    /// Construct an empty bounded table.
    pub fn new(config: AnnounceFreshnessConfig) -> Result<Self, AnnounceFreshnessConfigError> {
        Ok(Self {
            config: config.validate()?,
            rows: BTreeMap::new(),
            by_age: BTreeMap::new(),
            next_order: 0,
        })
    }

    /// The table's declared bounds.
    pub const fn config(&self) -> AnnounceFreshnessConfig {
        self.config
    }

    /// Change bounds without resetting retained freshness state.
    ///
    /// Shrinking trims each row's oldest blobs and evicts rows by their oldest acceptance.
    /// Invalid zero capacities leave the table unchanged.
    pub fn reconfigure(
        &mut self,
        config: AnnounceFreshnessConfig,
    ) -> Result<AnnounceFreshnessReconfigure, AnnounceFreshnessConfigError> {
        self.config = config.validate()?;
        let mut evicted_blobs = 0;
        for row in self.rows.values_mut() {
            let excess = row.blobs.len().saturating_sub(config.blob_capacity);
            row.blobs.drain(..excess);
            evicted_blobs += excess;
        }
        let mut evicted_destinations = Vec::new();
        while self.rows.len() > config.destination_capacity {
            evicted_destinations.extend(self.evict_oldest());
        }
        Ok(AnnounceFreshnessReconfigure {
            evicted_blobs,
            evicted_destinations,
        })
    }

    /// Decide whether a candidate may change caller-owned announce state.
    ///
    /// `route_live` is whether the caller holds an unexpired route to the candidate's
    /// destination. Without one, any retained row belongs to a route that is gone and the
    /// candidate is a first sighting. This method is pure: an accepted result is not durable
    /// until the caller invokes [`Self::record_accepted`].
    pub fn evaluate(
        &self,
        candidate: AnnounceFreshnessCandidate,
        route_live: bool,
    ) -> AnnounceFreshnessDecision {
        let Some(row) = self.rows.get(&candidate.destination).filter(|_| route_live) else {
            return AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting);
        };
        if candidate.timebase() > row.timebase {
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::NewerTimebase)
        } else if row.blobs.contains(&candidate.blob) {
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
        } else {
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase)
        }
    }

    /// Record a candidate under the reason [`Self::evaluate`] admitted it.
    ///
    /// This intentionally does not re-evaluate: callers can decline an otherwise acceptable
    /// announce when address-book admission fails. Under the caller's packet serialization
    /// guard, record after that admission and before route mutation, publication, or relay,
    /// and drop the route of any evicted destination.
    pub fn record_accepted(
        &mut self,
        candidate: AnnounceFreshnessCandidate,
        accepted: AnnounceFreshnessAccept,
    ) -> AnnounceFreshnessRecord {
        let order = self.take_order();
        let mut record = AnnounceFreshnessRecord {
            evicted_blob: None,
            evicted_destination: None,
        };
        if let Some(row) = self.rows.get_mut(&candidate.destination) {
            self.by_age.remove(&row.order);
            if accepted == AnnounceFreshnessAccept::FirstSighting {
                row.blobs.clear();
            }
            if row.blobs.len() >= self.config.blob_capacity {
                record.evicted_blob = row.blobs.pop_front();
            }
            row.blobs.push_back(candidate.blob);
            row.timebase = candidate.timebase();
            row.order = order;
        } else {
            if self.rows.len() >= self.config.destination_capacity {
                record.evicted_destination = self.evict_oldest();
            }
            self.rows.insert(
                candidate.destination,
                Row {
                    timebase: candidate.timebase(),
                    blobs: VecDeque::from([candidate.blob]),
                    order,
                },
            );
        }
        self.by_age.insert(order, candidate.destination);
        record
    }

    fn evict_oldest(&mut self) -> Option<AddressHash> {
        let (_, destination) = self.by_age.pop_first()?;
        self.rows.remove(&destination);
        Some(destination)
    }

    fn take_order(&mut self) -> u64 {
        // A saturated sequence would collide in `by_age`. Renumber the bounded live set in
        // its existing order first; it cannot hold enough rows to exhaust `u64` ranks.
        if self.next_order == u64::MAX {
            let ordered = core::mem::take(&mut self.by_age);
            for (rank, destination) in (0..).zip(ordered.into_values()) {
                if let Some(row) = self.rows.get_mut(&destination) {
                    row.order = rank;
                }
                self.by_age.insert(rank, destination);
            }
            self.next_order = self.by_age.len() as u64;
        }
        let order = self.next_order;
        self.next_order += 1;
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announce::ANNOUNCE_NONCE_LEN;

    const DEST_A: AddressHash = AddressHash::from_bytes([0xa1; 16]);
    const DEST_B: AddressHash = AddressHash::from_bytes([0xb2; 16]);
    const DEST_C: AddressHash = AddressHash::from_bytes([0xc3; 16]);
    const LIVE: bool = true;
    const GONE: bool = false;

    fn blob(nonce: u8, timebase: u64) -> AnnounceBlob {
        AnnounceBlob::mint([nonce; ANNOUNCE_NONCE_LEN], timebase).expect("test timebase fits")
    }

    fn candidate(destination: AddressHash, nonce: u8, timebase: u64) -> AnnounceFreshnessCandidate {
        AnnounceFreshnessCandidate {
            destination,
            blob: blob(nonce, timebase),
        }
    }

    fn table(destination_capacity: usize, blob_capacity: usize) -> AnnounceFreshness {
        AnnounceFreshness::new(AnnounceFreshnessConfig {
            destination_capacity,
            blob_capacity,
        })
        .expect("nonzero capacities")
    }

    /// Evaluate and, if admitted, record: what a caller does when nothing else refuses.
    fn admit(
        freshness: &mut AnnounceFreshness,
        input: AnnounceFreshnessCandidate,
        route_live: bool,
    ) -> AnnounceFreshnessDecision {
        let decision = freshness.evaluate(input, route_live);
        if let AnnounceFreshnessDecision::Accept(accepted) = decision {
            freshness.record_accepted(input, accepted);
        }
        decision
    }

    #[test]
    fn rejects_zero_capacities() {
        assert!(matches!(
            AnnounceFreshness::new(AnnounceFreshnessConfig {
                destination_capacity: 0,
                blob_capacity: 1,
            }),
            Err(AnnounceFreshnessConfigError::ZeroDestinationCapacity)
        ));
        assert!(matches!(
            AnnounceFreshness::new(AnnounceFreshnessConfig {
                destination_capacity: 1,
                blob_capacity: 0,
            }),
            Err(AnnounceFreshnessConfigError::ZeroBlobCapacity)
        ));
    }

    #[test]
    fn evaluate_is_pure_until_the_caller_records() {
        let freshness = table(1, 2);
        let input = candidate(DEST_A, 1, 10);
        for _ in 0..2 {
            assert_eq!(
                freshness.evaluate(input, LIVE),
                AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting)
            );
        }
    }

    /// RNS 1.5.7 `Transport.py` 2221-2290 with gravity and unresponsive paths set aside: a
    /// live row admits only a later emission, whatever the hops; no row admits anything.
    #[test]
    fn decision_table_follows_rns_emission_order_and_route_liveness() {
        let timebases = [99, 100, 101];
        let nonces = [1, 2];
        let mut cells = 0;
        for timebase in timebases {
            for nonce in nonces {
                for route_live in [LIVE, GONE] {
                    let mut freshness = table(1, 8);
                    freshness.record_accepted(
                        candidate(DEST_A, 1, 100),
                        AnnounceFreshnessAccept::FirstSighting,
                    );
                    let got = freshness.evaluate(candidate(DEST_A, nonce, timebase), route_live);
                    let expected = if !route_live {
                        AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting)
                    } else if timebase > 100 {
                        AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::NewerTimebase)
                    } else if nonce == 1 && timebase == 100 {
                        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
                    } else {
                        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase)
                    };
                    assert_eq!(
                        got, expected,
                        "timebase={timebase}, nonce={nonce}, route_live={route_live}"
                    );
                    cells += 1;
                }
            }
        }
        assert_eq!(cells, 12);
    }

    #[test]
    fn a_live_route_orders_by_its_newest_emission() {
        let mut freshness = table(1, 4);
        let first = candidate(DEST_A, 1, 10);
        let newest = candidate(DEST_A, 2, 12);
        assert!(admit(&mut freshness, first, LIVE).is_accepted());
        assert!(admit(&mut freshness, newest, LIVE).is_accepted());

        // Emitted between the two accepted announces: newer than the first, not the newest.
        assert_eq!(
            freshness.evaluate(candidate(DEST_A, 3, 11), LIVE),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase)
        );
        assert_eq!(
            freshness.evaluate(first, LIVE),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
        );
        assert_eq!(
            freshness.evaluate(candidate(DEST_A, 4, 13), LIVE),
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::NewerTimebase)
        );
    }

    #[test]
    fn a_same_blob_after_route_loss_is_a_first_sighting_that_resets_history() {
        let mut freshness = table(1, 4);
        let cached = candidate(DEST_A, 1, 20);
        assert!(admit(&mut freshness, cached, LIVE).is_accepted());
        assert!(admit(&mut freshness, candidate(DEST_A, 2, 30), LIVE).is_accepted());

        // The route expired. A transport answering a path request from its cache sends the
        // blob it holds, which may be older than what this receiver last saw.
        assert_eq!(
            admit(&mut freshness, cached, GONE),
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting)
        );
        // The culled row's blobs went with it, so ordering restarts from the restored route.
        assert_eq!(
            freshness.evaluate(cached, LIVE),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
        );
        assert_eq!(
            freshness.evaluate(candidate(DEST_A, 3, 25), LIVE),
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::NewerTimebase)
        );
    }

    #[test]
    fn blob_capacity_trims_history_without_weakening_emission_order() {
        let mut freshness = table(1, 2);
        let first = candidate(DEST_A, 1, 1);
        freshness.record_accepted(first, AnnounceFreshnessAccept::FirstSighting);
        freshness.record_accepted(
            candidate(DEST_A, 2, 2),
            AnnounceFreshnessAccept::NewerTimebase,
        );
        let record = freshness.record_accepted(
            candidate(DEST_A, 3, 3),
            AnnounceFreshnessAccept::NewerTimebase,
        );

        assert_eq!(record.evicted_blob, Some(first.blob));
        assert_eq!(freshness.rows[&DEST_A].blobs.len(), 2);
        assert_eq!(
            freshness.evaluate(first, LIVE),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase),
            "a trimmed blob is still refused, now as stale"
        );
    }

    #[test]
    fn destination_capacity_evicts_the_oldest_accepted_row() {
        let mut freshness = table(2, 2);
        admit(&mut freshness, candidate(DEST_A, 1, 1), GONE);
        admit(&mut freshness, candidate(DEST_B, 1, 1), GONE);
        // Refreshing A makes B the oldest acceptance.
        admit(&mut freshness, candidate(DEST_A, 2, 2), LIVE);
        let record = freshness.record_accepted(
            candidate(DEST_C, 1, 1),
            AnnounceFreshnessAccept::FirstSighting,
        );

        assert_eq!(record.evicted_destination, Some(DEST_B));
        assert_eq!(freshness.rows.len(), 2);
        assert_eq!(freshness.by_age.len(), 2);
        assert!(freshness.rows.contains_key(&DEST_A));
        assert!(freshness.rows.contains_key(&DEST_C));
    }

    #[test]
    fn reconfigure_keeps_valid_state_and_deterministically_trims_new_bounds() {
        let mut freshness = table(3, 3);
        admit(&mut freshness, candidate(DEST_A, 1, 1), GONE);
        admit(&mut freshness, candidate(DEST_A, 2, 2), LIVE);
        admit(&mut freshness, candidate(DEST_B, 1, 1), GONE);

        let outcome = freshness
            .reconfigure(AnnounceFreshnessConfig {
                destination_capacity: 1,
                blob_capacity: 1,
            })
            .expect("valid replacement bounds");
        assert_eq!(outcome.evicted_blobs, 1);
        assert_eq!(outcome.evicted_destinations, [DEST_A]);
        assert_eq!(freshness.rows.len(), 1);
        assert!(freshness.rows.contains_key(&DEST_B));

        let prior = freshness.config();
        assert_eq!(
            freshness.reconfigure(AnnounceFreshnessConfig {
                destination_capacity: 0,
                ..prior
            }),
            Err(AnnounceFreshnessConfigError::ZeroDestinationCapacity)
        );
        assert_eq!(
            freshness.config(),
            prior,
            "invalid replacement leaves state intact"
        );
    }

    #[test]
    fn default_profiles_have_bounded_logical_state_payloads() {
        fn logical_payload(destinations: usize, blobs_per_destination: usize) -> usize {
            core::mem::size_of::<AnnounceFreshness>()
                + destinations
                    * (core::mem::size_of::<(AddressHash, Row)>()
                        + core::mem::size_of::<(u64, AddressHash)>()
                        + blobs_per_destination * core::mem::size_of::<AnnounceBlob>())
        }

        let board_32x8 = logical_payload(32, 8);
        let host_4096x16 = logical_payload(4_096, 16);

        // Payload-only accounting: the table header plus every retained key, row and blob.
        // B-tree node slack, allocator metadata and spare capacity are allocator/target facts
        // and are deliberately not presented as a measured heap upper bound.
        assert!(
            board_32x8 <= 16 * 1024,
            "32x8 logical payload was {board_32x8} bytes"
        );
        assert!(
            host_4096x16 <= 2 * 1024 * 1024,
            "4096x16 logical payload was {host_4096x16} bytes"
        );
    }

    #[test]
    fn acceptance_order_rebases_before_counter_exhaustion() {
        let mut freshness = table(2, 2);
        admit(&mut freshness, candidate(DEST_A, 1, 1), GONE);
        admit(&mut freshness, candidate(DEST_B, 1, 1), GONE);
        freshness.next_order = u64::MAX;

        // Refresh B at the artificial rollover boundary. A must remain the oldest row and
        // therefore be the one displaced when C arrives.
        admit(&mut freshness, candidate(DEST_B, 2, 2), LIVE);
        let record = freshness.record_accepted(
            candidate(DEST_C, 1, 1),
            AnnounceFreshnessAccept::FirstSighting,
        );
        assert_eq!(record.evicted_destination, Some(DEST_A));
    }
}
