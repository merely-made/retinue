//! Announce rebroadcast for both cores: RNS's announce table and per-interface announce cap,
//! bounded and clock-free. Times are the caller's millisecond ticks.
//!
//! The table holds each relayed announce for a jittered first transmission and one retry,
//! and drops it early once neighbours are heard doing the work (`Transport.py` 765-829,
//! 2180-2203, 2338). The cap queues relayed announces an interface has no airtime for, at a
//! share of its bitrate, [`CapRate`] (`Transport.py` 1522-1585, `Interface.py` 391-427).

use alloc::vec::Vec;

use crate::hash::AddressHash;
use crate::node::{
    ANNOUNCE_CAP_PERCENT, FIRST_HOP_ALLOWANCE_BITS, InterfaceId, LOCAL_REBROADCASTS_MAX,
    QUEUED_ANNOUNCE_LIFE, REBROADCAST_GRACE, REBROADCAST_WINDOW,
};
use crate::packet::Packet;

/// Transmissions per announce: the first and RNS's single `PATHFINDER_R` retry. RNS ends an
/// entry once `retries` reaches two (`Transport.py` 772-777).
const TRANSMISSIONS: u8 = 2;

/// A relayed announce, ready for the wire: hops already counted, this node stamped as its
/// transport.
#[derive(Debug, Clone)]
pub(crate) struct Rebroadcast {
    pub(crate) destination: AddressHash,
    pub(crate) packet: Packet,
    /// Where it was heard. A single-radio shell sends it back out there.
    pub(crate) interface: InterfaceId,
    /// The announce's emission timebase, so a queue keeps the newest of a destination.
    pub(crate) emitted: u64,
}

#[derive(Debug)]
struct Entry {
    rebroadcast: Rebroadcast,
    due: u64,
    sent: u8,
    /// Neighbour rebroadcasts heard at our hop count (RNS `IDX_AT_LCL_RBRD`).
    heard: u8,
}

/// RNS's announce table, bounded.
#[derive(Debug)]
pub(crate) struct Rebroadcasts {
    entries: Vec<Entry>,
    capacity: usize,
}

impl Rebroadcasts {
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::new(),
            capacity,
        }
    }

    /// Schedule a first transmission `delay` after `now`, replacing any entry for the same
    /// destination as RNS does. At capacity an entry already sent once makes room; with none,
    /// the announce is refused rather than displacing one still waiting for its first send.
    ///
    /// A replacement keeps the old entry's send count: the newer announce takes over the
    /// remaining transmission, so re-announcing one destination cannot pin a slot as unsent
    /// and starve the table (RNS resets its retries; its table is unbounded).
    pub(crate) fn schedule(&mut self, rebroadcast: Rebroadcast, now: u64, delay: u64) -> bool {
        let mut entry = Entry {
            rebroadcast,
            due: now.saturating_add(delay),
            sent: 0,
            heard: 0,
        };
        let destination = entry.rebroadcast.destination;
        if let Some(existing) = self.find(destination) {
            entry.sent = self.entries[existing].sent;
            self.entries[existing] = entry;
            return true;
        }
        if self.entries.len() >= self.capacity {
            let Some(index) = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.sent > 0)
                .min_by_key(|(_, entry)| entry.due)
                .map(|(index, _)| index)
            else {
                return false;
            };
            self.entries.swap_remove(index);
        }
        self.entries.push(entry);
        true
    }

    /// A relayed copy of `destination`'s announce was heard with `hops` on the wire. Returns
    /// whether that ended our rebroadcast: two neighbours relaying at our hop count, or one
    /// node passing ours on within the retry grace, after our first send (`Transport.py`
    /// 2183-2203).
    pub(crate) fn heard(&mut self, destination: AddressHash, hops: u8, now: u64) -> bool {
        let Some(index) = self.find(destination) else {
            return false;
        };
        let entry = &mut self.entries[index];
        let ours = entry.rebroadcast.packet.hops;
        let mut done = false;
        if hops == ours {
            entry.heard = entry.heard.saturating_add(1);
            done = entry.sent > 0 && entry.heard >= LOCAL_REBROADCASTS_MAX;
        }
        if u16::from(hops) == u16::from(ours) + 1 && entry.sent > 0 && now < entry.due {
            done = true;
        }
        if done {
            self.entries.swap_remove(index);
        }
        done
    }

    /// [`Self::heard`] for an announce turned away unverified, as a neighbour's relay of the
    /// one we hold is (it repeats the blob, so freshness calls it a replay). It counts only if
    /// its signed fields match the copy that verified, so it needs no signature check.
    pub(crate) fn heard_copy(&mut self, packet: &Packet, now: u64) -> bool {
        let Some(index) = self.find(packet.destination) else {
            return false;
        };
        let held = &self.entries[index].rebroadcast.packet;
        if held.destination_type != packet.destination_type
            || held.context_flag != packet.context_flag
            || held.payload != packet.payload
        {
            return false;
        }
        self.heard(packet.destination, packet.hops, now)
    }

    /// Take the next transmission due at `now`. The entry stays for its retry, after
    /// [`REBROADCAST_GRACE`] plus [`REBROADCAST_WINDOW`], and leaves after its last send.
    pub(crate) fn pop_due(&mut self, now: u64) -> Option<Rebroadcast> {
        let index = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.due <= now)
            .min_by_key(|(_, entry)| entry.due)
            .map(|(index, _)| index)?;
        let entry = &mut self.entries[index];
        entry.sent += 1;
        entry.due = now
            .saturating_add(REBROADCAST_GRACE)
            .saturating_add(REBROADCAST_WINDOW);
        if entry.sent >= TRANSMISSIONS {
            Some(self.entries.swap_remove(index).rebroadcast)
        } else {
            Some(entry.rebroadcast.clone())
        }
    }

    /// When [`Self::pop_due`] next has something.
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.entries.iter().map(|entry| entry.due).min()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, destination: AddressHash) -> bool {
        self.find(destination).is_some()
    }

    fn find(&self, destination: AddressHash) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.rebroadcast.destination == destination)
    }
}

/// One interface's announce budget: the airtime of one 500-byte MTU and the share of it
/// announces may use, in percent (`Reticulum.py` 286-289). The airtime is the fraction
/// `mtu_ms / per` milliseconds, so a configured bitrate paces exactly (RNS works in float
/// seconds) rather than from a whole-millisecond [`first_hop_airtime`](crate::node::first_hop_airtime).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CapRate {
    mtu_ms: u64,
    per: u64,
    percent: u64,
}

impl CapRate {
    /// From a carrier's MTU airtime in milliseconds.
    pub(crate) const fn from_airtime(mtu_ms: u64, percent: u64) -> Self {
        Self {
            mtu_ms,
            per: 1,
            percent,
        }
    }

    /// From a bitrate in bits per second, which must be nonzero.
    pub(crate) const fn from_bitrate(bps: u64, percent: u64) -> Self {
        Self {
            mtu_ms: FIRST_HOP_ALLOWANCE_BITS * 1_000,
            per: bps,
            percent,
        }
    }

    /// From a carrier's MTU airtime, at RNS's default share, [`ANNOUNCE_CAP_PERCENT`].
    pub(crate) const fn new(mtu_ms: u64) -> Self {
        Self::from_airtime(mtu_ms, ANNOUNCE_CAP_PERCENT)
    }
}

/// The wait after sending `len` bytes before the next relayed announce, in whole milliseconds
/// rounded up: the transmission time over the cap share (`Transport.py` 1526-1528). A zero
/// share counts as 1 %.
pub(crate) fn cap_wait(len: usize, rate: CapRate) -> u64 {
    let num = len as u128 * 100 * u128::from(rate.mtu_ms);
    let den = 500 * u128::from(rate.percent.max(1)) * u128::from(rate.per.max(1));
    u64::try_from(num.div_ceil(den)).unwrap_or(u64::MAX)
}

/// What [`AnnounceCap::offer`] did with an announce.
#[derive(Debug)]
pub(crate) enum Offer {
    /// Within the cap: send it now.
    Send(Packet),
    /// Queued, or merged into a queued announce for the same destination.
    Queued,
    /// The queue was full.
    Dropped,
}

#[derive(Debug)]
struct Queued {
    rebroadcast: Rebroadcast,
    at: u64,
}

/// One interface's relayed-announce budget and queue.
#[derive(Debug)]
pub(crate) struct AnnounceCap {
    allowed_at: u64,
    queue: Vec<Queued>,
    capacity: usize,
}

impl AnnounceCap {
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            allowed_at: 0,
            queue: Vec::new(),
            capacity,
        }
    }

    /// Send now if nothing is queued and the budget allows, else queue. A destination already
    /// queued keeps one entry, replaced by a newer emission (`Transport.py` 1531-1565).
    pub(crate) fn offer(&mut self, rebroadcast: Rebroadcast, now: u64, rate: CapRate) -> Offer {
        if self.queue.is_empty() && now >= self.allowed_at {
            self.allowed_at = now.saturating_add(cap_wait(rebroadcast.packet.encoded_len(), rate));
            return Offer::Send(rebroadcast.packet);
        }
        if let Some(queued) = self
            .queue
            .iter_mut()
            .find(|queued| queued.rebroadcast.destination == rebroadcast.destination)
        {
            if rebroadcast.emitted > queued.rebroadcast.emitted {
                *queued = Queued {
                    rebroadcast,
                    at: now,
                };
            }
            return Offer::Queued;
        }
        if self.queue.len() >= self.capacity {
            return Offer::Dropped;
        }
        self.queue.push(Queued {
            rebroadcast,
            at: now,
        });
        Offer::Queued
    }

    /// Release the queued announce with the fewest hops, oldest first, once the budget allows.
    /// Entries older than [`QUEUED_ANNOUNCE_LIFE`] are dropped (`Interface.py` 391-427).
    pub(crate) fn pop_due(&mut self, now: u64, rate: CapRate) -> Option<Packet> {
        if now < self.allowed_at {
            return None;
        }
        self.queue
            .retain(|queued| now <= queued.at.saturating_add(QUEUED_ANNOUNCE_LIFE));
        let index = self
            .queue
            .iter()
            .enumerate()
            .min_by_key(|(_, queued)| (queued.rebroadcast.packet.hops, queued.at))
            .map(|(index, _)| index)?;
        let packet = self.queue.remove(index).rebroadcast.packet;
        self.allowed_at = now.saturating_add(cap_wait(packet.encoded_len(), rate));
        Some(packet)
    }

    /// When [`Self::pop_due`] next has something.
    pub(crate) fn next_due(&self) -> Option<u64> {
        (!self.queue.is_empty()).then_some(self.allowed_at)
    }

    /// Whether this cap holds nothing and constrains nothing at `now`, so it can be forgotten.
    pub(crate) fn idle(&self, now: u64) -> bool {
        self.queue.is_empty() && now >= self.allowed_at
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{DestinationType, HeaderType, PacketType, Propagation};

    fn rebroadcast(seed: u8, hops: u8, emitted: u64) -> Rebroadcast {
        let destination = AddressHash::from_bytes([seed; 16]);
        Rebroadcast {
            destination,
            packet: Packet {
                ifac: false,
                header_type: HeaderType::Type2,
                context_flag: false,
                propagation: Propagation::Broadcast,
                destination_type: DestinationType::Single,
                packet_type: PacketType::Announce,
                hops,
                transport: Some(AddressHash::from_bytes([0xEE; 16])),
                destination,
                context: 0,
                payload: alloc::vec![seed; 132],
            },
            interface: 0,
            emitted,
        }
    }

    #[test]
    fn sends_once_then_retries_once_after_the_grace() {
        let mut table = Rebroadcasts::new(4);
        assert!(table.schedule(rebroadcast(1, 1, 0), 100, 300));
        assert!(table.pop_due(399).is_none());
        assert!(table.pop_due(400).is_some());
        let retry = 400 + REBROADCAST_GRACE + REBROADCAST_WINDOW;
        assert_eq!(table.next_due(), Some(retry));
        assert!(table.pop_due(retry - 1).is_none());
        assert!(table.pop_due(retry).is_some());
        assert_eq!(table.len(), 0, "two transmissions, then done");
    }

    #[test]
    fn neighbour_rebroadcasts_end_ours_only_after_our_first_send() {
        let mut table = Rebroadcasts::new(4);
        table.schedule(rebroadcast(1, 2, 0), 0, 100);
        assert!(!table.heard(AddressHash::from_bytes([1; 16]), 2, 50));
        assert!(!table.heard(AddressHash::from_bytes([1; 16]), 2, 60));
        assert!(table.pop_due(100).is_some(), "RNS still sends once");
        assert!(table.heard(AddressHash::from_bytes([1; 16]), 2, 200));
        assert_eq!(table.len(), 0);

        table.schedule(rebroadcast(2, 2, 0), 0, 0);
        table.pop_due(0);
        assert!(!table.heard(AddressHash::from_bytes([2; 16]), 4, 10));
        assert!(
            table.heard(AddressHash::from_bytes([2; 16]), 3, 10),
            "passed on with one more hop"
        );
    }

    #[test]
    fn an_unverified_copy_is_heard_only_if_it_matches_the_held_announce() {
        let mut table = Rebroadcasts::new(4);
        let held = rebroadcast(1, 2, 0);
        table.schedule(held.clone(), 0, 0);
        table.pop_due(0);
        let mut other = held.packet.clone();
        other.payload[0] ^= 1;
        assert!(!table.heard_copy(&other, 10), "different signed fields");
        let mut copy = held.packet.clone();
        copy.transport = Some(AddressHash::from_bytes([0xDD; 16]));
        copy.hops = 3;
        assert!(table.heard_copy(&copy, 10), "passed on with one more hop");
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn a_full_table_makes_room_only_from_sent_entries() {
        let mut table = Rebroadcasts::new(1);
        assert!(table.schedule(rebroadcast(1, 1, 0), 0, 10));
        assert!(!table.schedule(rebroadcast(2, 1, 0), 0, 10));
        table.pop_due(10);
        assert!(table.schedule(rebroadcast(2, 1, 0), 10, 10));
    }

    #[test]
    fn a_replacement_takes_over_the_remaining_transmission() {
        let mut table = Rebroadcasts::new(1);
        assert!(table.schedule(rebroadcast(1, 1, 0), 0, 0));
        table.pop_due(0);
        assert!(
            table.schedule(rebroadcast(1, 2, 1), 10, 10),
            "a newer announce"
        );
        assert!(
            table.schedule(rebroadcast(2, 1, 0), 10, 10),
            "the replaced entry stays evictable"
        );
        assert!(!table.contains(AddressHash::from_bytes([1; 16])));

        let mut table = Rebroadcasts::new(4);
        table.schedule(rebroadcast(3, 1, 0), 0, 0);
        table.pop_due(0);
        table.schedule(rebroadcast(3, 1, 1), 10, 10);
        assert_eq!(table.pop_due(20).unwrap().emitted, 1);
        assert_eq!(table.len(), 0, "sent once more, then done");
    }

    #[test]
    fn an_extreme_airtime_saturates() {
        assert_eq!(cap_wait(500, CapRate::new(u64::MAX)), u64::MAX);
        let mut cap = AnnounceCap::new(1);
        assert!(matches!(
            cap.offer(rebroadcast(1, 1, 0), 5, CapRate::new(u64::MAX)),
            Offer::Send(_)
        ));
        assert!(matches!(
            cap.offer(rebroadcast(2, 1, 0), 6, CapRate::new(u64::MAX)),
            Offer::Queued
        ));
        assert!(cap.pop_due(u64::MAX - 1, CapRate::new(u64::MAX)).is_none());
    }

    #[test]
    fn the_share_scales_the_wait() {
        // 1200 bps at 5 %: a 300-byte announce takes 2 s, so 40 s.
        let rate = CapRate::from_bitrate(1_200, 5);
        assert_eq!(cap_wait(300, rate), 40_000);
        assert_eq!(cap_wait(300, CapRate::from_bitrate(1_200, 2)), 100_000);
        assert_eq!(
            cap_wait(300, CapRate::from_bitrate(1_200, 0)),
            cap_wait(300, CapRate::from_bitrate(1_200, 1))
        );
    }

    #[test]
    fn a_fast_bitrate_paces_finer_than_its_mtu_airtime() {
        // 100 Mbps at 2 %: a 167-byte announce takes 13.36 us, so 0.668 ms, one tick; a
        // whole-millisecond MTU airtime would make it 17 ms.
        assert_eq!(cap_wait(167, CapRate::from_bitrate(100_000_000, 2)), 1);
        assert_eq!(cap_wait(167, CapRate::new(1)), 17);
    }

    #[test]
    fn the_cap_queues_the_newest_emission_and_releases_fewest_hops_first() {
        // 64 ms per MTU (62.5 kbps): a 167-byte announce takes 21.4 ms, so 1069 ms at 2 %.
        const RATE: CapRate = CapRate::new(64);
        let mut cap = AnnounceCap::new(4);
        let first = rebroadcast(1, 3, 10);
        let wait = cap_wait(first.packet.encoded_len(), RATE);
        assert_eq!(wait, 1069);
        assert!(matches!(cap.offer(first, 0, RATE), Offer::Send(_)));
        assert!(matches!(
            cap.offer(rebroadcast(2, 3, 10), 1, RATE),
            Offer::Queued
        ));
        assert!(matches!(
            cap.offer(rebroadcast(3, 1, 10), 2, RATE),
            Offer::Queued
        ));
        assert!(matches!(
            cap.offer(rebroadcast(2, 5, 11), 3, RATE),
            Offer::Queued
        ));
        assert!(matches!(
            cap.offer(rebroadcast(2, 4, 9), 4, RATE),
            Offer::Queued
        ));
        assert_eq!(cap.queued(), 2);
        assert_eq!(cap.next_due(), Some(wait));
        assert!(cap.pop_due(wait - 1, RATE).is_none());
        assert_eq!(cap.pop_due(wait, RATE).unwrap().hops, 1);
        assert!(cap.pop_due(wait + 1, RATE).is_none());
        let newest = cap.pop_due(2 * wait, RATE).unwrap();
        assert_eq!(newest.hops, 5, "the newer emission replaced the queued one");
        assert_eq!(cap.next_due(), None);
    }
}
