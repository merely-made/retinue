//! Bounded, per-destination announce freshness admission.
//!
//! This is deliberately separate from packet-loop suppression. Callers first
//! [`AnnounceFreshness::evaluate`] a verified announce, perform any fallible
//! admission that must leave no trace on refusal, and then
//! [`AnnounceFreshness::record_accepted`] it before route mutation, publication,
//! or relay scheduling.
//! The caller supplies its own tick and route TTL, keeping this module
//! `no_std + alloc` and making its clock policy explicit.

use alloc::vec::Vec;

use crate::announce::AnnounceBlob;
use crate::hash::AddressHash;

/// Bounds and retention policy for [`AnnounceFreshness`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessConfig {
    /// Maximum number of destinations with retained freshness state.
    pub destination_capacity: usize,
    /// Maximum retained announce blobs for each destination.
    pub blob_capacity: usize,
    /// How long an accepted entry remains replay-protected, in caller ticks.
    ///
    /// A zero lifetime deliberately disables retention.  It is useful for a
    /// caller that needs the route comparison but elects not to retain replay
    /// state; normal users should choose a positive, explicit bound.
    pub retention_ticks: u64,
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

/// The packet-derived fields that participate in receive freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessCandidate {
    /// Announced destination.
    pub destination: AddressHash,
    /// Full nonce-and-timebase blob from the signed announce payload.
    pub blob: AnnounceBlob,
    /// Calibrated route hop count for this received copy.
    pub hops: u8,
}

impl AnnounceFreshnessCandidate {
    /// The candidate's 40-bit timebase, decoded from [`Self::blob`].
    pub const fn timebase(self) -> u64 {
        self.blob.timebase()
    }
}

/// Why a candidate was admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceFreshnessAccept {
    /// No retained state exists for this destination.
    FirstSighting,
    /// The blob is new and its timebase is strictly above the incumbent's.
    NewerTimebase,
    /// The incumbent route has expired and this new blob has a worse hop count.
    ///
    /// This counterintuitive branch is the observed compatibility rule.  Once
    /// recorded, the lower-timebase candidate is the new incumbent.
    ExpiredIncumbentWorseRoute,
}

/// Why a candidate was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnounceFreshnessReject {
    /// The exact full blob remains in bounded retained history.
    Replay,
    /// A new blob was neither newer nor the expired/worse-route exception.
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessRecord {
    /// A destination whose whole retained state aged out before this record.
    pub expired_destinations: usize,
    /// Historical blobs that aged out while their destination remained retained.
    pub expired_blobs: usize,
    /// An older retained blob removed because this destination reached its bound.
    pub evicted_blob: Option<AnnounceBlob>,
    /// A destination evicted to make room for a new destination.
    pub evicted_destination: Option<AddressHash>,
    /// Whether this record allocated a new destination entry.
    pub inserted_destination: bool,
}

/// What changing a table's configured bounds removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessReconfigure {
    /// Destination entries whose incumbent was outside the new retention window.
    pub expired_destinations: usize,
    /// Historical blobs removed by the new retention window.
    pub expired_blobs: usize,
    /// Historical blobs removed because the new blob bound was smaller.
    pub evicted_blobs: usize,
    /// Destination entries removed by the new destination bound.
    pub evicted_destinations: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Incumbent {
    blob: AnnounceBlob,
    timebase: u64,
    hops: u8,
    accepted_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RetainedBlob {
    blob: AnnounceBlob,
    accepted_at: u64,
}

#[derive(Clone, Debug)]
struct DestinationEntry {
    destination: AddressHash,
    incumbent: Incumbent,
    blobs: Vec<RetainedBlob>,
    // Acceptance order, rather than a wall-clock tick, makes bounded row
    // eviction deterministic even when a caller's tick moves backwards.
    accepted_order: u64,
}

/// A bounded, allocation-backed announce freshness table.
///
/// Retention applies to a destination's incumbent and its blob history as one
/// unit.  Once the incumbent reaches `retention_ticks`, the destination becomes
/// a first sighting again.  This makes the finite replay guarantee explicit and
/// lets an expired row release its destination slot before row eviction.
#[derive(Clone, Debug)]
pub struct AnnounceFreshness {
    config: AnnounceFreshnessConfig,
    entries: Vec<DestinationEntry>,
    next_accept_order: u64,
}

impl AnnounceFreshness {
    /// Construct an empty bounded table.
    pub fn new(config: AnnounceFreshnessConfig) -> Result<Self, AnnounceFreshnessConfigError> {
        if config.destination_capacity == 0 {
            return Err(AnnounceFreshnessConfigError::ZeroDestinationCapacity);
        }
        if config.blob_capacity == 0 {
            return Err(AnnounceFreshnessConfigError::ZeroBlobCapacity);
        }
        Ok(Self {
            config,
            entries: Vec::new(),
            next_accept_order: 0,
        })
    }

    /// The table's declared bounds and retention policy.
    pub const fn config(&self) -> AnnounceFreshnessConfig {
        self.config
    }

    /// Change bounds without resetting still-retained freshness state.
    ///
    /// The supplied `now` applies the new retention window.  If a smaller
    /// capacity requires eviction, history is trimmed in acceptance order and
    /// rows are evicted by their oldest acceptance event.  Invalid zero
    /// capacities leave the table unchanged.
    pub fn reconfigure(
        &mut self,
        config: AnnounceFreshnessConfig,
        now: u64,
    ) -> Result<AnnounceFreshnessReconfigure, AnnounceFreshnessConfigError> {
        if config.destination_capacity == 0 {
            return Err(AnnounceFreshnessConfigError::ZeroDestinationCapacity);
        }
        if config.blob_capacity == 0 {
            return Err(AnnounceFreshnessConfigError::ZeroBlobCapacity);
        }

        self.config = config;
        let expired_destinations = self.drop_expired_destinations(now);
        let retention_ticks = self.config.retention_ticks;
        let mut expired_blobs = 0;
        let mut evicted_blobs = 0;
        for entry in &mut self.entries {
            let before = entry.blobs.len();
            entry
                .blobs
                .retain(|stored| now.saturating_sub(stored.accepted_at) < retention_ticks);
            expired_blobs += before - entry.blobs.len();
            while entry.blobs.len() > self.config.blob_capacity {
                entry.blobs.remove(0);
                evicted_blobs += 1;
            }
        }

        let mut evicted_destinations = 0;
        while self.entries.len() > self.config.destination_capacity {
            let index = self.oldest_entry_index();
            self.entries.remove(index);
            evicted_destinations += 1;
        }
        Ok(AnnounceFreshnessReconfigure {
            expired_destinations,
            expired_blobs,
            evicted_blobs,
            evicted_destinations,
        })
    }

    /// Decide whether a candidate may change caller-owned announce state.
    ///
    /// This method is pure: an accepted result is not durable until the caller
    /// invokes [`Self::record_accepted`].  `route_ttl == 0` means an incumbent
    /// is immediately route-expired.  Tick deltas use saturating arithmetic, so
    /// a backwards caller tick cannot make an incumbent or retained blob expire.
    pub fn evaluate(
        &self,
        candidate: AnnounceFreshnessCandidate,
        now: u64,
        route_ttl: u64,
    ) -> AnnounceFreshnessDecision {
        let Some(entry) = self.entry_if_retained(candidate.destination, now) else {
            return AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting);
        };

        if entry.incumbent.blob == candidate.blob
            || entry.blobs.iter().any(|stored| {
                stored.blob == candidate.blob && self.is_retained(stored.accepted_at, now)
            })
        {
            return AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay);
        }

        if candidate.timebase() > entry.incumbent.timebase {
            return AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::NewerTimebase);
        }

        let route_expired = now.saturating_sub(entry.incumbent.accepted_at) >= route_ttl;
        if route_expired && candidate.hops > entry.incumbent.hops {
            return AnnounceFreshnessDecision::Accept(
                AnnounceFreshnessAccept::ExpiredIncumbentWorseRoute,
            );
        }

        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase)
    }

    /// Record an already-admitted candidate after address-book admission succeeds.
    ///
    /// This intentionally does not re-evaluate: callers can decline an otherwise
    /// acceptable announce when address-book admission fails.  Under the caller's
    /// packet serialization guard, record after that admission and before route
    /// mutation, publication, or relay so those later effects share one committed
    /// freshness result. Calling this for a rejected candidate violates the
    /// evaluate-then-record contract and is a caller bug.
    pub fn record_accepted(
        &mut self,
        candidate: AnnounceFreshnessCandidate,
        accepted_at: u64,
    ) -> AnnounceFreshnessRecord {
        let expired_destinations = self.drop_expired_destinations(accepted_at);
        let expired_blobs = self.drop_expired_blobs(accepted_at);
        let accepted_order = self.take_accept_order();

        if let Some(index) = self.entry_index(candidate.destination) {
            let entry = &mut self.entries[index];
            let evicted_blob = if entry.blobs.len() == self.config.blob_capacity {
                Some(entry.blobs.remove(0).blob)
            } else {
                None
            };
            entry.blobs.push(RetainedBlob {
                blob: candidate.blob,
                accepted_at,
            });
            entry.incumbent = Incumbent {
                blob: candidate.blob,
                timebase: candidate.timebase(),
                hops: candidate.hops,
                accepted_at,
            };
            entry.accepted_order = accepted_order;
            return AnnounceFreshnessRecord {
                expired_destinations,
                expired_blobs,
                evicted_blob,
                evicted_destination: None,
                inserted_destination: false,
            };
        }

        let evicted_destination = if self.entries.len() == self.config.destination_capacity {
            let index = self.oldest_entry_index();
            Some(self.entries.remove(index).destination)
        } else {
            None
        };
        self.entries.push(DestinationEntry {
            destination: candidate.destination,
            incumbent: Incumbent {
                blob: candidate.blob,
                timebase: candidate.timebase(),
                hops: candidate.hops,
                accepted_at,
            },
            blobs: alloc::vec![RetainedBlob {
                blob: candidate.blob,
                accepted_at,
            }],
            accepted_order,
        });
        AnnounceFreshnessRecord {
            expired_destinations,
            expired_blobs,
            evicted_blob: None,
            evicted_destination,
            inserted_destination: true,
        }
    }

    fn entry_if_retained(&self, destination: AddressHash, now: u64) -> Option<&DestinationEntry> {
        self.entries.iter().find(|entry| {
            entry.destination == destination && self.is_retained(entry.incumbent.accepted_at, now)
        })
    }

    fn entry_index(&self, destination: AddressHash) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.destination == destination)
    }

    fn is_retained(&self, accepted_at: u64, now: u64) -> bool {
        now.saturating_sub(accepted_at) < self.config.retention_ticks
    }

    fn drop_expired_destinations(&mut self, now: u64) -> usize {
        let before = self.entries.len();
        let retention_ticks = self.config.retention_ticks;
        self.entries
            .retain(|entry| now.saturating_sub(entry.incumbent.accepted_at) < retention_ticks);
        before - self.entries.len()
    }

    fn drop_expired_blobs(&mut self, now: u64) -> usize {
        let retention_ticks = self.config.retention_ticks;
        let mut removed = 0;
        for entry in &mut self.entries {
            let before = entry.blobs.len();
            entry
                .blobs
                .retain(|stored| now.saturating_sub(stored.accepted_at) < retention_ticks);
            removed += before - entry.blobs.len();
        }
        removed
    }

    fn take_accept_order(&mut self) -> u64 {
        // A saturated sequence would give every later acceptance the same age and make row
        // eviction depend on vector position. Rebase the bounded live set before allocating
        // that value. Sorting is deterministic because all orders are unique until this
        // branch, and the table cannot contain enough allocated rows to exhaust `u64` ranks.
        if self.next_accept_order == u64::MAX {
            self.entries.sort_by_key(|entry| entry.accepted_order);
            for (rank, entry) in self.entries.iter_mut().enumerate() {
                entry.accepted_order =
                    u64::try_from(rank).expect("allocated freshness rows fit in u64");
            }
            self.next_accept_order =
                u64::try_from(self.entries.len()).expect("allocated freshness rows fit in u64");
        }
        let order = self.next_accept_order;
        self.next_accept_order += 1;
        order
    }

    fn oldest_entry_index(&self) -> usize {
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.accepted_order)
            .map(|(index, _)| index)
            .expect("entry capacity checked before eviction")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announce::ANNOUNCE_NONCE_LEN;

    const DEST_A: AddressHash = AddressHash::from_bytes([0xa1; 16]);
    const DEST_B: AddressHash = AddressHash::from_bytes([0xb2; 16]);
    const DEST_C: AddressHash = AddressHash::from_bytes([0xc3; 16]);

    fn blob(nonce: u8, timebase: u64) -> AnnounceBlob {
        AnnounceBlob::mint([nonce; ANNOUNCE_NONCE_LEN], timebase).expect("test timebase fits")
    }

    fn candidate(
        destination: AddressHash,
        nonce: u8,
        timebase: u64,
        hops: u8,
    ) -> AnnounceFreshnessCandidate {
        AnnounceFreshnessCandidate {
            destination,
            blob: blob(nonce, timebase),
            hops,
        }
    }

    fn table(
        destination_capacity: usize,
        blob_capacity: usize,
        retention_ticks: u64,
    ) -> AnnounceFreshness {
        AnnounceFreshness::new(AnnounceFreshnessConfig {
            destination_capacity,
            blob_capacity,
            retention_ticks,
        })
        .expect("nonzero capacities")
    }

    #[test]
    fn rejects_zero_capacities() {
        assert!(matches!(
            AnnounceFreshness::new(AnnounceFreshnessConfig {
                destination_capacity: 0,
                blob_capacity: 1,
                retention_ticks: 1,
            }),
            Err(AnnounceFreshnessConfigError::ZeroDestinationCapacity)
        ));
        assert!(matches!(
            AnnounceFreshness::new(AnnounceFreshnessConfig {
                destination_capacity: 1,
                blob_capacity: 0,
                retention_ticks: 1,
            }),
            Err(AnnounceFreshnessConfigError::ZeroBlobCapacity)
        ));
    }

    #[test]
    fn evaluate_is_pure_until_the_caller_records() {
        let freshness = table(1, 2, 100);
        let input = candidate(DEST_A, 1, 10, 2);

        assert_eq!(
            freshness.evaluate(input, 1, 10),
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting)
        );
        assert_eq!(
            freshness.evaluate(input, 1, 10),
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting)
        );
    }

    #[test]
    fn p8_decision_table_has_all_72_expected_model_cells() {
        #[derive(Clone, Copy, Debug)]
        enum TimebaseCase {
            Older,
            Equal,
            Newer,
        }
        #[derive(Clone, Copy, Debug)]
        enum NonceCase {
            Same,
            New,
        }
        #[derive(Clone, Copy, Debug)]
        enum HopCase {
            Better,
            Equal,
            Worse,
        }
        #[derive(Clone, Copy, Debug)]
        enum Context {
            Ordinary,
            PathResponse,
        }

        let timebases = [
            (TimebaseCase::Older, 99),
            (TimebaseCase::Equal, 100),
            (TimebaseCase::Newer, 101),
        ];
        let nonces = [(NonceCase::Same, 1), (NonceCase::New, 2)];
        let hops = [
            (HopCase::Better, 4),
            (HopCase::Equal, 5),
            (HopCase::Worse, 6),
        ];
        // Context is intentionally not part of `AnnounceFreshnessCandidate`. Repeating the
        // model cell for both observed contexts pins that absence; Endpoint packet tests carry
        // the separate signed-packet evidence that context changes do not bypass admission.
        let contexts = [Context::Ordinary, Context::PathResponse];
        let route_states = [(false, 109_u64), (true, 110_u64)];
        let mut cells = 0;

        for (timebase_case, timebase) in timebases {
            for (nonce_case, nonce) in nonces {
                for (hop_case, candidate_hops) in hops {
                    for context in contexts {
                        for (expired, now) in route_states {
                            let mut freshness = table(1, 8, 1_000);
                            freshness.record_accepted(candidate(DEST_A, 1, 100, 5), 100);
                            let input = candidate(DEST_A, nonce, timebase, candidate_hops);
                            let got = freshness.evaluate(input, now, 10);
                            let exact_replay = nonce == 1 && timebase == 100;
                            let expected = if exact_replay {
                                AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
                            } else if timebase > 100 {
                                AnnounceFreshnessDecision::Accept(
                                    AnnounceFreshnessAccept::NewerTimebase,
                                )
                            } else if expired && candidate_hops > 5 {
                                AnnounceFreshnessDecision::Accept(
                                    AnnounceFreshnessAccept::ExpiredIncumbentWorseRoute,
                                )
                            } else {
                                AnnounceFreshnessDecision::Reject(
                                    AnnounceFreshnessReject::StaleTimebase,
                                )
                            };
                            assert_eq!(
                                got, expected,
                                "timebase={timebase_case:?}, nonce={nonce_case:?}, hops={hop_case:?}, context={context:?}, expired={expired}"
                            );
                            cells += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(cells, 72);
    }

    #[test]
    fn historical_replay_rejects_after_a_newer_incumbent() {
        let mut freshness = table(1, 4, 100);
        let first = candidate(DEST_A, 1, 10, 2);
        let newer = candidate(DEST_A, 2, 11, 2);
        freshness.record_accepted(first, 1);
        assert!(freshness.evaluate(newer, 2, 10).is_accepted());
        freshness.record_accepted(newer, 2);

        assert_eq!(
            freshness.evaluate(first, 3, 10),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
        );
    }

    #[test]
    fn expired_worse_route_rollback_becomes_incumbent_and_keeps_history() {
        let mut freshness = table(1, 4, 100);
        let high = candidate(DEST_A, 1, 20, 2);
        let rollback = candidate(DEST_A, 2, 10, 3);
        freshness.record_accepted(high, 10);

        assert_eq!(
            freshness.evaluate(rollback, 20, 10),
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::ExpiredIncumbentWorseRoute)
        );
        freshness.record_accepted(rollback, 20);
        assert_eq!(
            freshness.evaluate(high, 21, 10),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
        );
        assert_eq!(
            freshness.entries[0].incumbent.timebase, 10,
            "the accepted lower-timebase candidate is now incumbent"
        );
    }

    #[test]
    fn retention_expiry_releases_a_destination_before_recording() {
        let mut freshness = table(1, 2, 5);
        let first = candidate(DEST_A, 1, 10, 2);
        freshness.record_accepted(first, 10);

        assert_eq!(
            freshness.evaluate(first, 15, 10),
            AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting)
        );
        let record = freshness.record_accepted(first, 15);
        assert_eq!(record.expired_destinations, 1);
        assert!(record.inserted_destination);
    }

    #[test]
    fn blob_capacity_evicts_the_oldest_accepted_blob() {
        let mut freshness = table(1, 2, 100);
        let first = candidate(DEST_A, 1, 1, 1);
        let second = candidate(DEST_A, 2, 2, 1);
        let third = candidate(DEST_A, 3, 3, 1);
        freshness.record_accepted(first, 1);
        freshness.record_accepted(second, 2);
        let record = freshness.record_accepted(third, 3);

        assert_eq!(record.evicted_blob, Some(first.blob));
        assert_eq!(freshness.entries[0].blobs.len(), 2);
        assert_eq!(freshness.entries[0].blobs[0].blob, second.blob);
        assert_eq!(freshness.entries[0].blobs[1].blob, third.blob);
    }

    #[test]
    fn destination_capacity_evicts_the_oldest_accepted_row() {
        let mut freshness = table(2, 2, 100);
        freshness.record_accepted(candidate(DEST_A, 1, 1, 1), 1);
        freshness.record_accepted(candidate(DEST_B, 1, 1, 1), 2);
        let record = freshness.record_accepted(candidate(DEST_C, 1, 1, 1), 3);

        assert_eq!(record.evicted_destination, Some(DEST_A));
        assert_eq!(freshness.entries.len(), 2);
        assert_eq!(freshness.entries[0].destination, DEST_B);
        assert_eq!(freshness.entries[1].destination, DEST_C);
    }

    #[test]
    fn reconfigure_keeps_valid_state_and_deterministically_trims_new_bounds() {
        let mut freshness = table(3, 3, 100);
        let first = candidate(DEST_A, 1, 1, 1);
        let second = candidate(DEST_A, 2, 2, 1);
        let third = candidate(DEST_B, 1, 1, 1);
        freshness.record_accepted(first, 1);
        freshness.record_accepted(second, 2);
        freshness.record_accepted(third, 3);

        let outcome = freshness
            .reconfigure(
                AnnounceFreshnessConfig {
                    destination_capacity: 1,
                    blob_capacity: 1,
                    retention_ticks: 100,
                },
                4,
            )
            .expect("valid replacement bounds");
        assert_eq!(outcome.expired_destinations, 0);
        assert_eq!(outcome.expired_blobs, 0);
        assert_eq!(outcome.evicted_blobs, 1);
        assert_eq!(outcome.evicted_destinations, 1);
        assert_eq!(freshness.entries.len(), 1);
        assert_eq!(freshness.entries[0].destination, DEST_B);

        let prior = freshness.config();
        assert_eq!(
            freshness.reconfigure(
                AnnounceFreshnessConfig {
                    destination_capacity: 0,
                    ..prior
                },
                4,
            ),
            Err(AnnounceFreshnessConfigError::ZeroDestinationCapacity)
        );
        assert_eq!(
            freshness.config(),
            prior,
            "invalid replacement leaves state intact"
        );
    }

    #[test]
    fn reconfigure_reports_retention_expiry_separately_from_capacity_eviction() {
        let mut freshness = table(1, 3, 100);
        freshness.record_accepted(candidate(DEST_A, 1, 1, 1), 1);
        freshness.record_accepted(candidate(DEST_A, 2, 2, 1), 10);

        let outcome = freshness
            .reconfigure(
                AnnounceFreshnessConfig {
                    destination_capacity: 1,
                    blob_capacity: 3,
                    retention_ticks: 5,
                },
                12,
            )
            .expect("valid replacement bounds");
        assert_eq!(outcome.expired_destinations, 0);
        assert_eq!(outcome.expired_blobs, 1);
        assert_eq!(outcome.evicted_blobs, 0);
        assert_eq!(outcome.evicted_destinations, 0);
    }

    #[test]
    fn default_profiles_have_bounded_logical_state_payloads() {
        fn logical_payload(destinations: usize, blobs_per_destination: usize) -> usize {
            core::mem::size_of::<AnnounceFreshness>()
                + destinations
                    * (core::mem::size_of::<DestinationEntry>()
                        + blobs_per_destination * core::mem::size_of::<RetainedBlob>())
        }

        let board_32x8 = logical_payload(32, 8);
        let host_4096x16 = logical_payload(4_096, 16);

        // These are payload-only accounting bounds: the table and Vec headers plus every
        // retained element. Allocator metadata and spare capacity are allocator/target facts
        // and are deliberately not presented as a measured heap upper bound.
        assert!(
            board_32x8 <= 16 * 1024,
            "32x8 logical payload was {board_32x8} bytes"
        );
        assert!(
            host_4096x16 <= 2 * 1024 * 1024,
            "4096x16 logical payload was {host_4096x16} bytes"
        );
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(board_32x8, 8_760);
            assert_eq!(host_4096x16, 1_900_600);
        }
    }

    #[test]
    fn backwards_now_uses_saturating_arithmetic() {
        let mut freshness = table(1, 2, 100);
        freshness.record_accepted(candidate(DEST_A, 1, 100, 2), 100);

        assert_eq!(
            freshness.evaluate(candidate(DEST_A, 2, 99, 3), 1, 10),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase),
            "backwards now must not fabricate route expiry"
        );
        assert_eq!(
            freshness.evaluate(candidate(DEST_A, 1, 100, 2), 1, 10),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay),
            "backwards now must not discard retained history"
        );
    }

    #[test]
    fn acceptance_order_rebases_before_counter_exhaustion() {
        let mut freshness = table(2, 2, 100);
        freshness.record_accepted(candidate(DEST_A, 1, 1, 1), 1);
        freshness.record_accepted(candidate(DEST_B, 1, 1, 1), 2);
        freshness.next_accept_order = u64::MAX;

        // Refresh B at the artificial rollover boundary. A must remain the oldest row and
        // therefore be the one displaced when C arrives.
        freshness.record_accepted(candidate(DEST_B, 2, 2, 1), 3);
        let record = freshness.record_accepted(candidate(DEST_C, 1, 1, 1), 4);
        assert_eq!(record.evicted_destination, Some(DEST_A));
    }
}
