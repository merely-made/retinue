//! The event loop and the handler for each scheduled item.

use retinue::announce::{ANNOUNCE_NONCE_LEN, AnnounceBlob};
use retinue::node::Action;
use retinue::packet::{Packet, PacketType};

use super::state::{Scheduled, Sim, derive64, ordered};
use super::{RADIO, SimError};
use crate::trace::{Event, Refusal, Trace};

impl<'a, const P: usize, const A: usize, const L: usize, const R: usize> Sim<'a, P, A, L, R> {
    pub(super) fn run(mut self) -> Result<Trace, SimError> {
        while let Some(((t, _), item)) = self.queue.pop_first() {
            match item {
                Scheduled::Cut(i) => self.on_cut(t, i),
                Scheduled::Send(i) => self.on_send(t, i)?,
                Scheduled::Poll(n) => self.on_poll(t, n)?,
                Scheduled::Wake(n) => self.on_wake(t, n)?,
                Scheduled::Deliver { frame, node } => self.on_deliver(t, frame, node)?,
            }
            for n in 0..self.nodes.len() {
                self.arm_wake(t, n);
            }
        }
        Ok(self.finish())
    }

    fn on_cut(&mut self, t: u64, i: usize) {
        let cut = &self.scenario.cuts[i];
        let (a, b) = (self.index(&cut.a), self.index(&cut.b));
        self.cut.insert(ordered(a, b));
        self.events.push(Event::Cut {
            t,
            a: cut.a.clone(),
            b: cut.b.clone(),
        });
    }

    fn on_poll(&mut self, t: u64, n: usize) -> Result<(), SimError> {
        let blob = if self.nodes[n].node.announce_due(t) {
            let node = &mut self.nodes[n];
            node.draws += 1;
            let seed = derive64(b"retinue-sim/announce-nonce", &node.name, node.draws);
            let mut nonce = [0_u8; ANNOUNCE_NONCE_LEN];
            nonce.copy_from_slice(&seed[..ANNOUNCE_NONCE_LEN]);
            Some(AnnounceBlob::mint(nonce, t / 1_000).map_err(|_| SimError::Timebase)?)
        } else {
            None
        };
        let actions = self.nodes[n].node.poll(t, RADIO, blob.as_ref());
        self.perform(t, n, actions, None)?;
        self.schedule(t + self.scenario.timing.poll_interval, Scheduled::Poll(n));
        Ok(())
    }

    /// Poll a node at its next rebroadcast, as a shell does between regular polls.
    fn arm_wake(&mut self, t: u64, n: usize) {
        let Some(at) = self.nodes[n].node.next_rebroadcast() else {
            return;
        };
        let at = at.max(t);
        if self.nodes[n].wake != Some(at) {
            self.nodes[n].wake = Some(at);
            self.schedule(at, Scheduled::Wake(n));
        }
    }

    fn on_wake(&mut self, t: u64, n: usize) -> Result<(), SimError> {
        // A wake superseded by an earlier one is spent.
        if self.nodes[n].wake != Some(t) {
            return Ok(());
        }
        self.nodes[n].wake = None;
        let actions = self.nodes[n].node.poll(t, RADIO, None);
        self.perform(t, n, actions, None)
    }

    fn on_send(&mut self, t: u64, i: usize) -> Result<(), SimError> {
        let send = &self.scenario.sends[i];
        let (from, to) = (self.index(&send.from), self.index(&send.to));
        let destination = self.nodes[to].destination;
        let node = &mut self.nodes[from];
        node.draws += 1;
        let seed = derive64(b"retinue-sim/link-seed", &node.name, node.draws);
        let hop = node.node.next_hop(destination, t);
        let message = i as u32;
        let Some(actions) = node.node.open_link(destination, RADIO, &seed, t) else {
            let reason = if node.node.peers().knows(destination) {
                Refusal::PendingFull
            } else {
                Refusal::UnknownDestination
            };
            self.events.push(Event::SendRefused {
                t,
                message,
                from: send.from.clone(),
                to: send.to.clone(),
                reason,
            });
            return Ok(());
        };
        // Requests `open_link` dropped as overdue happened first, so they are recorded
        // before the send that freed their slots, each with the state between the two.
        let actions = self.checked(t, from, actions)?;
        let (expired, actions): (Vec<_>, Vec<_>) = actions
            .into_iter()
            .partition(|action| matches!(action, Action::LinkRequestTimedOut { .. }));
        let request = actions.iter().find_map(|action| match action {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        });
        for action in expired {
            if let Action::LinkRequestTimedOut { link_id } = action {
                self.link_request_expired(t, from, link_id, u32::from(request.is_some()));
            }
        }
        self.messages[i].link = request
            .as_ref()
            .and_then(|packet| retinue::link::link_id(packet).ok());
        let via = request
            .as_ref()
            .and_then(|packet| packet.transport)
            .map(|hash| self.name_of_identity(hash));
        self.events.push(Event::Send {
            t,
            message,
            from: send.from.clone(),
            to: send.to.clone(),
            via,
            hops: hop.map(|hop| hop.hops),
        });
        self.perform_list(t, from, actions, None)
    }

    fn on_deliver(&mut self, t: u64, frame: u32, n: usize) -> Result<(), SimError> {
        let bytes = self.frames[frame as usize].bytes.clone();
        let packet = Packet::decode(&bytes).map_err(|_| SimError::Undecodable { frame })?;
        let node = &mut self.nodes[n];
        node.face.rx_frames = node.face.rx_frames.saturating_add(1);
        node.face.last_rx_len = Some(bytes.len() as u16);
        if packet.packet_type == PacketType::Announce {
            node.heard_announces.entry(packet.hash()).or_insert(frame);
        }
        let actions = node.node.ingest(RADIO, &packet, t);
        self.perform(t, n, actions, Some((frame, packet.hash())))
    }
}
