//! Carrying out a node's actions: transmissions, deliveries and expired requests.

use retinue::hash::AddressHash;
use retinue::node::{Action, Actions};
use retinue::packet::{Packet, PacketType};

use super::state::{Frame, Scheduled, Sim, derive64, ordered};
use super::{RADIO, SimError};
use crate::trace::{Delivery, Effect, Event, FaceEventKind, Origin};

impl<'a, const P: usize, const A: usize, const L: usize, const R: usize> Sim<'a, P, A, L, R> {
    /// One call's actions, refused if its `ACTIONS` bound overflowed.
    pub(super) fn checked(
        &self,
        t: u64,
        n: usize,
        actions: Actions<A>,
    ) -> Result<Vec<Action>, SimError> {
        if actions.overflowed() != 0 {
            return Err(SimError::ActionsOverflowed {
                node: self.nodes[n].name.clone(),
                t,
            });
        }
        Ok(actions.into_iter().collect())
    }

    /// Carry out one call's actions. `heard` is the frame that produced them, if any.
    pub(super) fn perform(
        &mut self,
        t: u64,
        n: usize,
        actions: Actions<A>,
        heard: Option<(u32, AddressHash)>,
    ) -> Result<(), SimError> {
        let actions = self.checked(t, n, actions)?;
        self.perform_list(t, n, actions, heard)
    }

    pub(super) fn perform_list(
        &mut self,
        t: u64,
        n: usize,
        actions: Vec<Action>,
        heard: Option<(u32, AddressHash)>,
    ) -> Result<(), SimError> {
        let mut effects = Vec::new();
        let mut sends = Vec::new();
        let mut links_up = Vec::new();
        let mut deliveries = Vec::new();
        for action in actions {
            match action {
                Action::Send { packet, .. } => sends.push(packet),
                Action::Learned { destination } => effects.push(Effect::Learned {
                    destination: self.name_of_destination(destination),
                }),
                Action::LinkUp { link_id } => {
                    self.note(n, FaceEventKind::Info, "link up");
                    effects.push(Effect::LinkUp {
                        link: link_id.to_string(),
                    });
                    links_up.push(link_id);
                }
                Action::LinkDown { link_id } => {
                    self.note(n, FaceEventKind::Info, "link down");
                    effects.push(Effect::LinkDown {
                        link: link_id.to_string(),
                    });
                }
                Action::LinkRequestTimedOut { link_id } => {
                    self.link_request_expired(t, n, link_id, 0);
                }
                Action::Data { link_id, payload } => {
                    let message = self.message_on(link_id);
                    effects.push(Effect::Data {
                        link: link_id.to_string(),
                        len: payload.len() as u32,
                        message,
                    });
                    if let Some(m) = message
                        && self.scenario.sends[m as usize].to == self.nodes[n].name
                        && let Some((frame, _)) = heard
                    {
                        deliveries.push((m, frame));
                    }
                }
                Action::Resource { link_id, data } => effects.push(Effect::Resource {
                    link: link_id.to_string(),
                    len: data.len() as u32,
                }),
            }
        }
        if let Some((frame, _)) = heard {
            let state = self.state(t, n);
            self.events.push(Event::Receive {
                t,
                frame,
                node: self.nodes[n].name.clone(),
                effects,
                state,
            });
        }
        for packet in sends {
            let (origin, cause) = match heard {
                None if packet.packet_type == PacketType::Announce => (Origin::Announce, None),
                None => (Origin::App, None),
                Some((frame, hash)) if packet.hash() == hash => (Origin::Forward, Some(frame)),
                Some((frame, _)) => (Origin::Reply, Some(frame)),
            };
            self.transmit(t, n, packet, origin, cause);
        }
        for (m, frame) in deliveries {
            let path = self.path(frame, n);
            self.messages[m as usize].delivered = Some(Delivery {
                t,
                path: path.clone(),
            });
            self.events.push(Event::Delivered {
                t,
                message: m,
                node: self.nodes[n].name.clone(),
                path,
            });
        }
        for link_id in links_up {
            self.send_payload(t, n, link_id)?;
        }
        Ok(())
    }

    /// Record a link request node `n` dropped unanswered, with the node's state at expiry.
    /// `added` counts requests the same call added after the drop: `open_link` drops overdue
    /// requests and then adds its own in one call, so they are taken back out (Ruling 76).
    pub(super) fn link_request_expired(
        &mut self,
        t: u64,
        n: usize,
        link_id: AddressHash,
        added: u32,
    ) {
        // As the channel node notes it (`radio_hand::channel::node::LINK_UNANSWERED`).
        self.note(n, FaceEventKind::Failed, "link unanswered");
        let mut state = self.state(t, n);
        state.pending_links = state
            .pending_links
            .checked_sub(added)
            .expect("an added request is pending");
        self.events.push(Event::LinkRequestExpired {
            t,
            node: self.nodes[n].name.clone(),
            link: link_id.to_string(),
            message: self.message_on(link_id),
            state,
        });
    }

    /// The application half: once a send's link is up at its sender, carry the payload.
    fn send_payload(&mut self, t: u64, n: usize, link_id: AddressHash) -> Result<(), SimError> {
        let Some(m) = self.message_on(link_id) else {
            return Ok(());
        };
        if self.scenario.sends[m as usize].from != self.nodes[n].name {
            return Ok(());
        }
        let node = &mut self.nodes[n];
        node.draws += 1;
        let seed = derive64(b"retinue-sim/iv", &node.name, node.draws);
        let mut iv = [0_u8; retinue::token::IV_LEN];
        iv.copy_from_slice(&seed[..retinue::token::IV_LEN]);
        let payload = self.scenario.sends[m as usize].payload.as_bytes();
        if let Some(actions) = node.node.send(link_id, RADIO, payload, &iv) {
            self.perform(t, n, actions, None)?;
        }
        Ok(())
    }

    fn transmit(&mut self, t: u64, n: usize, packet: Packet, origin: Origin, cause: Option<u32>) {
        let bytes = packet.encode();
        let frame = self.frames.len() as u32;
        let mut heard_by = Vec::new();
        let mut blocked = Vec::new();
        for &other in &self.neighbours[n] {
            if self.cut.contains(&ordered(n, other)) {
                blocked.push(other);
            } else {
                heard_by.push(other);
            }
        }
        let node = &mut self.nodes[n];
        node.face.tx_frames = node.face.tx_frames.saturating_add(1);
        node.face.last_tx_len = Some(bytes.len() as u16);
        let summary = self.summary(&packet, bytes.len());
        let state = self.state(t, n);
        self.events.push(Event::Transmit {
            t,
            frame,
            node: self.nodes[n].name.clone(),
            origin,
            cause,
            packet: summary,
            heard_by: heard_by
                .iter()
                .map(|&i| self.nodes[i].name.clone())
                .collect(),
            blocked: blocked
                .iter()
                .map(|&i| self.nodes[i].name.clone())
                .collect(),
            state,
        });
        self.frames.push(Frame {
            bytes,
            node: n,
            origin,
            cause,
        });
        let arrival = t + self.scenario.timing.hop_delay;
        for other in heard_by {
            self.schedule(arrival, Scheduled::Deliver { frame, node: other });
        }
    }

    /// The forwarding chain that carried `frame` to node `n`, sender first.
    fn path(&self, frame: u32, n: usize) -> Vec<String> {
        let mut path = vec![self.nodes[n].name.clone()];
        let mut at = Some(frame);
        while let Some(id) = at {
            let record = &self.frames[id as usize];
            path.push(self.nodes[record.node].name.clone());
            at = match record.origin {
                Origin::Forward => record.cause,
                _ => None,
            };
        }
        path.reverse();
        path
    }
}
