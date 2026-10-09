//! Usage-based cleaning of known destinations and received ratchets
//! (RNS `Identity.py` 285-349 and 446-482).

use super::AddressBook;
use crate::hash::AddressHash;

/// RNS `Transport.DESTINATION_TIMEOUT`, one week, in seconds.
pub const DESTINATION_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;
/// RNS `Transport.UNUSED_DESTINATION_LINGER`, in seconds.
pub const UNUSED_DESTINATION_LINGER_SECS: u64 = 6 * 60;
/// RNS `Identity.RATCHET_EXPIRY`, 30 days, in seconds.
pub const RATCHET_EXPIRY_SECS: u64 = 30 * 24 * 60 * 60;

/// How long unused peers and received ratchets are kept, in the caller's ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// A never-used peer is dropped this long after its last announce.
    pub unused_linger: u64,
    /// A used peer is dropped this long after its last use.
    pub unused_timeout: u64,
    /// A received ratchet is forgotten this long after it was first heard.
    pub ratchet_expiry: u64,
}

impl Retention {
    /// RNS's periods for a caller whose tick is `1 / ticks_per_second`. The unused timeout is
    /// `DESTINATION_TIMEOUT * 1.25`, as RNS cleans (`Identity.py` 326).
    pub const fn rns(ticks_per_second: u64) -> Self {
        Self {
            unused_linger: UNUSED_DESTINATION_LINGER_SECS.saturating_mul(ticks_per_second),
            unused_timeout: (DESTINATION_TIMEOUT_SECS * 5 / 4).saturating_mul(ticks_per_second),
            ratchet_expiry: RATCHET_EXPIRY_SECS.saturating_mul(ticks_per_second),
        }
    }
}

/// What [`AddressBook::clean`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanReceipt {
    pub removed: usize,
    pub ratchets_expired: usize,
}

impl AddressBook {
    /// Forget peers that are neither retained nor `in_use` (RNS: have a path) and have gone
    /// unused past `retention`, and expire received ratchets past theirs.
    ///
    /// A never-used peer lingers [`Retention::unused_linger`] after its last announce. RNS
    /// 1.5.7 measures a never-used peer's idle time from zero, so it drops one at the first
    /// clean whatever the linger (`Identity.py` 318-326); this keeps the linger.
    pub fn clean(
        &mut self,
        now: u64,
        retention: &Retention,
        in_use: impl Fn(AddressHash) -> bool,
    ) -> CleanReceipt {
        let before = self.peers.len();
        self.peers.retain(|destination, peer| {
            let idle = if peer.last_used == 0 {
                now.saturating_sub(peer.last_heard) > retention.unused_linger
            } else {
                now.saturating_sub(peer.last_used) > retention.unused_timeout
            };
            peer.retained || !idle || in_use(*destination)
        });
        let mut ratchets_expired = 0;
        for peer in self.peers.values_mut() {
            if peer.ratchet.is_some()
                && now.saturating_sub(peer.ratchet_received) > retention.ratchet_expiry
            {
                peer.ratchet = None;
                ratchets_expired += 1;
            }
        }
        CleanReceipt {
            removed: before - self.peers.len(),
            ratchets_expired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::announce_at;
    use super::*;

    const POLICY: Retention = Retention {
        unused_linger: 10,
        unused_timeout: 100,
        ratchet_expiry: 1_000,
    };

    #[test]
    fn unused_peers_are_cleaned_by_usage_unless_retained_or_in_use() {
        let mut book = AddressBook::new();
        let [never, used, retained, routed] =
            [1, 2, 3, 4].map(|byte| announce_at(&mut book, byte, None, 0));
        book.mark_used(used, 5);
        book.set_retained(retained, true, 5);

        // Past the linger only the never-used, unprotected peer goes.
        let receipt = book.clean(11, &POLICY, |d| d == routed);
        assert_eq!(receipt.removed, 1);
        assert!(!book.knows(never));
        assert!(book.knows(used) && book.knows(retained) && book.knows(routed));

        // Past the unused timeout the used one follows; the retained one stays until
        // released, which counts as a use.
        assert_eq!(book.clean(106, &POLICY, |d| d == routed).removed, 1);
        assert!(!book.knows(used) && book.knows(retained));
        book.set_retained(retained, false, 200);
        assert_eq!(book.clean(300, &POLICY, |d| d == routed).removed, 0);
        assert_eq!(book.clean(301, &POLICY, |d| d == routed).removed, 1);
        assert!(book.knows(routed));
    }

    #[test]
    fn a_received_ratchet_expires_from_when_it_was_first_heard() {
        let mut book = AddressBook::new();
        let dest = announce_at(&mut book, 7, Some([1; 32]), 0);
        book.set_retained(dest, true, 0);
        // The same ratchet again keeps its first-heard tick; an announce without one keeps it.
        announce_at(&mut book, 7, Some([1; 32]), 600);
        announce_at(&mut book, 7, None, 700);
        assert_eq!(book.resolve(dest).unwrap().ratchet, Some([1; 32]));
        assert_eq!(book.clean(1_001, &POLICY, |_| false).ratchets_expired, 1);
        assert_eq!(book.resolve(dest).unwrap().ratchet, None);

        // A new ratchet restarts the expiry.
        announce_at(&mut book, 7, Some([2; 32]), 1_500);
        assert_eq!(book.clean(2_500, &POLICY, |_| false).ratchets_expired, 0);
        assert_eq!(book.resolve(dest).unwrap().ratchet_received, 1_500);
    }

    #[test]
    fn rns_periods_scale_by_tick_rate() {
        let millis = Retention::rns(1_000);
        assert_eq!(millis.unused_linger, 360_000);
        assert_eq!(millis.unused_timeout, 756_000_000);
        assert_eq!(millis.ratchet_expiry, 2_592_000_000);
    }
}
