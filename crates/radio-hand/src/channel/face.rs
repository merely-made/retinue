//! The four panels, built from the node's own state.
//!
//! Gate N6's substance: the same [`HostSnapshot`] shape a host used to project onto the
//! screen is now genuine, sourced from the board. The channel publishes it on every beat,
//! so the panels stay fresh with no host attached — which is the done condition.

use core::fmt::Write as _;

use radio_face::{
    DetailPolicy, EventKind, EventSource, HostSnapshot, IfacState, NodeSummary, PeerPath,
    PeerSummary, Personality, Text, UiEvent,
};
use retinue::hash::AddressHash;

use crate::channel::node::NodeChannel;

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize>
    NodeChannel<PEERS, ACTIONS, LINKS>
{
    /// Note that a peer announced, keeping the table bounded by evicting the stalest.
    pub(super) fn note_heard(&mut self, destination: AddressHash, now: u64) {
        if let Some(entry) = self.heard.iter_mut().find(|(d, _)| *d == destination) {
            entry.1 = now;
            return;
        }
        if self.heard.is_full()
            && let Some(stalest) = self
                .heard
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(index, _)| index)
        {
            self.heard.swap_remove(stalest);
        }
        let _ = self.heard.push((destination, now));
    }

    /// The face's four panels, built from this node's own state.
    ///
    /// Gate N6's point: the same [`HostSnapshot`] shape a host used to project is now
    /// *genuine*, sourced from the board. Published on every beat, so it stays fresh with
    /// no host attached — which is exactly the done condition.
    pub(super) fn face_snapshot(&self, now: u64) -> HostSnapshot {
        let dest = self.node.destination();
        let bytes = dest.as_slice();
        let mut address_tail = [0_u8; 8];
        address_tail.copy_from_slice(&bytes[..8]);
        let mut fingerprint = [0_u8; 16];
        fingerprint.copy_from_slice(bytes);

        // The three most recently heard peers, newest first, genuine ages.
        let mut recent: heapless::Vec<(AddressHash, u64), 8> = self.heard.clone();
        recent.sort_unstable_by_key(|entry| core::cmp::Reverse(entry.1));
        let mut peers: [Option<PeerSummary>; 3] = [None, None, None];
        for (slot, (destination, at)) in recent.iter().take(3).enumerate() {
            let mut name = Text::<12>::empty();
            for byte in &destination.as_slice()[..4] {
                let _ = write!(&mut name, "{byte:02x}");
            }
            peers[slot] = Some(PeerSummary {
                name,
                path: PeerPath::Direct,
                age_secs: (now.saturating_sub(*at) / 1_000) as u32,
            });
        }

        HostSnapshot {
            valid_for_secs: 15,
            personality: Personality::Retinue,
            detail: DetailPolicy::Named,
            node: Some(NodeSummary {
                name: Text::from_truncated("retinue.node"),
                address_tail,
                fingerprint,
                role: Text::from_truncated("node"),
                uptime_secs: (now / 1_000) as u32,
            }),
            link_count: self.node.link_count() as u8,
            admitted_links: self.node.link_count() as u8,
            queue_depth: self.unsent,
            ifac: IfacState::Off,
            peers,
            peer_overflow: self.node.peers().len().saturating_sub(3) as u8,
            event: self.last_event,
        }
    }

    /// A one-line face event, truncated to fit.
    pub(super) fn note_event(&mut self, kind: EventKind, text: &str) {
        self.last_event = Some(UiEvent {
            source: EventSource::Local,
            kind,
            text: Text::from_truncated(text),
        });
    }
}
