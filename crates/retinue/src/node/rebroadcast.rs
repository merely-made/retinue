//! Relayed announces: jittered scheduling, neighbour suppression and the announce cap.

use super::{Action, Actions, InterfaceId, Node, QUEUED_ANNOUNCES, REBROADCAST_WINDOW};
use crate::hash::AddressHash;
use crate::packet::Packet;
use crate::rebroadcast::{AnnounceCap, Offer, Rebroadcast};

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// When [`Node::poll`] next has a relayed announce to send, if any. A shell that polls
    /// slower than [`REBROADCAST_WINDOW`] can wake for it.
    pub fn next_rebroadcast(&self) -> Option<u64> {
        self.announce_caps
            .iter()
            .filter_map(|(_, cap)| cap.next_due())
            .chain(self.rebroadcasts.next_due())
            .min()
    }

    /// Hold a relayed announce for a first transmission within [`REBROADCAST_WINDOW`]
    /// (`Transport.py` 2338). The delay is drawn from the announce and this node's identity,
    /// so neighbours spread out without an RNG and a replay is reproducible.
    pub(super) fn schedule_rebroadcast(
        &mut self,
        interface: InterfaceId,
        packet: Packet,
        emitted: u64,
        now: u64,
    ) {
        let hash = packet.hash();
        let ours = self.identity.hash();
        let draw = u16::from_le_bytes([
            hash.as_bytes()[0] ^ ours.as_bytes()[0],
            hash.as_bytes()[1] ^ ours.as_bytes()[1],
        ]);
        let delay = u64::from(draw) % (REBROADCAST_WINDOW + 1);
        let rebroadcast = Rebroadcast {
            destination: packet.destination,
            packet,
            interface,
            emitted,
        };
        if !self.rebroadcasts.schedule(rebroadcast, now, delay) {
            self.transport_counters.refused_rebroadcasts = self
                .transport_counters
                .refused_rebroadcasts
                .saturating_add(1);
        }
    }

    /// A relayed copy of an announce we are rebroadcasting was heard (`Transport.py`
    /// 2180-2203).
    pub(super) fn hear_rebroadcast(&mut self, destination: AddressHash, hops: u8, now: u64) {
        if self.rebroadcasts.heard(destination, hops, now) {
            self.transport_counters.suppressed_rebroadcasts = self
                .transport_counters
                .suppressed_rebroadcasts
                .saturating_add(1);
        }
    }

    /// [`Self::hear_rebroadcast`] for an announce freshness turned away before verifying it:
    /// heard only as a copy of the announce we verified and hold.
    pub(super) fn hear_rebroadcast_copy(&mut self, packet: &Packet, now: u64) {
        if self.rebroadcasts.heard_copy(packet, now) {
            self.transport_counters.suppressed_rebroadcasts = self
                .transport_counters
                .suppressed_rebroadcasts
                .saturating_add(1);
        }
    }

    /// Send what the caps now allow, then what the table has due, while actions have room.
    pub(super) fn poll_rebroadcasts(&mut self, now: u64, actions: &mut Actions<ACTIONS>) {
        for index in 0..self.announce_caps.len() {
            let interface = self.announce_caps[index].0;
            let airtime = self.first_hop_airtime(interface);
            while actions.len() < ACTIONS {
                let Some(packet) = self.announce_caps[index].1.pop_due(now, airtime) else {
                    break;
                };
                self.send_rebroadcast(interface, packet, actions);
            }
        }
        while actions.len() < ACTIONS {
            let Some(rebroadcast) = self.rebroadcasts.pop_due(now) else {
                break;
            };
            self.offer_rebroadcast(rebroadcast, now, actions);
        }
        // An idle cap, or one whose airtime was cleared, holds nothing worth a slot.
        let first_hop_airtime = &self.first_hop_airtime;
        self.announce_caps.retain(|(interface, cap)| {
            !cap.idle(now) && first_hop_airtime.iter().any(|(id, _)| id == interface)
        });
    }

    /// Pass a due rebroadcast through its interface's cap, where the airtime is known
    /// (`Transport.py` 1522-1585). An interface without a cap slot sends uncapped.
    fn offer_rebroadcast(
        &mut self,
        rebroadcast: Rebroadcast,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        let interface = rebroadcast.interface;
        let airtime = self.first_hop_airtime(interface);
        if airtime == 0 {
            self.send_rebroadcast(interface, rebroadcast.packet, actions);
            return;
        }
        let index = match self
            .announce_caps
            .iter()
            .position(|(id, _)| *id == interface)
        {
            Some(index) => index,
            None => {
                if self
                    .announce_caps
                    .push((interface, AnnounceCap::new(QUEUED_ANNOUNCES)))
                    .is_err()
                {
                    self.send_rebroadcast(interface, rebroadcast.packet, actions);
                    return;
                }
                self.announce_caps.len() - 1
            }
        };
        match self.announce_caps[index].1.offer(rebroadcast, now, airtime) {
            Offer::Send(packet) => self.send_rebroadcast(interface, packet, actions),
            Offer::Queued => {
                self.transport_counters.capped_announces =
                    self.transport_counters.capped_announces.saturating_add(1);
            }
            Offer::Dropped => {
                self.transport_counters.dropped_announces =
                    self.transport_counters.dropped_announces.saturating_add(1);
            }
        }
    }

    fn send_rebroadcast(
        &mut self,
        interface: InterfaceId,
        packet: Packet,
        actions: &mut Actions<ACTIONS>,
    ) {
        if actions.push(Action::Send { interface, packet }) {
            self.transport_counters.forwarded_announces = self
                .transport_counters
                .forwarded_announces
                .saturating_add(1);
        }
    }
}
