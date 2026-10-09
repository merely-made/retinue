//! Forced local loss of a live resource, then a fresh link between the same nodes.

use retinue::node::{Action, InterruptionPermission};
use serde_json::{Value, json};
use std::collections::VecDeque;
use tulle::personality::ControllerState;
use tulle::personality_serial::PersonalitySerialRuntime;

use super::{Sessions, Side, random, send_packet, wire};
use crate::Result;

impl Sessions {
    /// Discard a live resource once the caller has settled its radio operation and
    /// accounted for every action it owns: a deliberate local-loss exercise. Sends the
    /// encrypted close through HOME, observes the peer's LinkDown, then reopens a fresh link
    /// from retained identities and peer knowledge without another announce.
    pub async fn forced_resource_interruption(
        &mut self,
        dut_radio: &mut PersonalitySerialRuntime,
        peer_radio: &mut PersonalitySerialRuntime,
        events: &mut Vec<Value>,
    ) -> Result<()> {
        if dut_radio.controller().state() != ControllerState::Home
            || peer_radio.controller().state() != ControllerState::Home
            || self.dut.link_count() != 1
            || self.peer.link_count() != 1
            || !self.dut.has_link(self.link)
            || !self.peer.has_link(self.link)
        {
            return Err("forced interruption requires one live link at HOME".into());
        }

        let old_link = self.link;
        let payload = random::<350>()?.to_vec();
        let started = self
            .dut
            .publish(old_link, 0, &payload, random()?, &random()?, self.now()?)
            .ok_or("forced interruption sender refused resource publication")?;
        let mut queue = VecDeque::new();
        let mut resources = Vec::new();
        self.queue_actions(Side::Dut, started, &mut queue, &mut resources)?;
        let offer = queue
            .pop_front()
            .ok_or("forced interruption resource offer missing")?;
        if offer.from != Side::Dut || offer.packet.context != retinue::link::CTX_RESOURCE_ADV {
            return Err("forced interruption did not begin with a DUT resource offer".into());
        }
        let (recipient, reply) = self
            .deliver_resource_packet(offer, dut_radio, peer_radio, events)
            .await?;
        if recipient != Side::Peer {
            return Err("forced interruption offer reached the wrong node".into());
        }
        self.queue_actions(recipient, reply, &mut queue, &mut resources)?;
        if queue.len() != 1
            || queue.front().is_none_or(|queued| {
                queued.from != Side::Peer
                    || queued.packet.context != retinue::link::CTX_RESOURCE_REQ
            })
            || !resources.is_empty()
            || !self.dut.transfer_active(old_link)
            || !self.peer.transfer_active(old_link)
        {
            return Err("forced interruption did not hold one peer resource request".into());
        }
        let sender = self.dut.pause_assessment();
        let receiver = self.peer.pause_assessment();
        if sender.outbound_resources != 1
            || sender.inbound_resources != 0
            || receiver.inbound_resources != 1
            || receiver.outbound_resources != 0
        {
            return Err("forced interruption resource ownership changed before loss".into());
        }
        events.push(
            json!({"kind":"forced_resource_held","link":format!("{old_link:?}"),
            "payload_size":payload.len(),"dut_outbound":sender.outbound_resources,
            "peer_inbound":receiver.inbound_resources,"queued_packets":queue.len()}),
        );

        let before = self.dut.pause_assessment();
        let mut entropy_called = false;
        if self
            .dut
            .force_interrupt(InterruptionPermission::PreserveSessions, || {
                entropy_called = true;
                [0_u8; 16]
            })
            .is_ok()
            || entropy_called
            || self.dut.pause_assessment() != before
            || !self.dut.has_link(old_link)
        {
            return Err("PreserveSessions changed local state or consumed close entropy".into());
        }
        events.push(
            json!({"kind":"forced_preserve_refused","link":format!("{old_link:?}"),
            "entropy_called":entropy_called,"links":self.dut.link_count()}),
        );

        // The peer's request was issued but never transmitted. It is caller-owned I/O:
        // discard and count it before local loss, since replaying it could create remote work.
        let cancelled = queue.len();
        let held = queue
            .pop_front()
            .ok_or("held resource request disappeared")?;
        if !queue.is_empty() || held.from != Side::Peer {
            return Err("forced interruption action queue accounting changed".into());
        }
        drop(held);
        events.push(
            json!({"kind":"forced_action_queue_cancelled","old_link":format!("{old_link:?}"),
            "cancelled":cancelled,"authority":"caller-owned unsent RF action"}),
        );

        let close_iv = random()?;
        let report = self
            .dut
            .force_interrupt(InterruptionPermission::AllowSessionLoss, || close_iv)
            .map_err(|_| "explicit session loss was refused")?;
        if report.closed_links.as_slice() != [old_link]
            || report.close_packets.len() != 1
            || !report.pending_links.is_empty()
            || !report.inbound_resources.is_empty()
            || report.outbound_resources.as_slice() != [old_link]
            || !report.transit_bridges.is_empty()
            || self.dut.has_link(old_link)
            || self.dut.transfer_active(old_link)
            || self.dut.link_count() != 0
        {
            return Err("forced interruption report did not exactly describe DUT loss".into());
        }
        events.push(json!({"kind":"forced_local_loss","old_link":format!("{old_link:?}"),
            "close_packets":report.close_packets.len(),"closed_links":report.closed_links.len(),
            "pending_links":report.pending_links.len(),"inbound_resources":report.inbound_resources.len(),
            "outbound_resources":report.outbound_resources.len(),"transit_bridges":report.transit_bridges.len()}));

        if self
            .dut
            .send(old_link, 0, b"old-link-must-fail", &random()?)
            .is_some()
        {
            return Err("locally discarded link still accepted application data".into());
        }
        let close = wire(
            dut_radio,
            peer_radio,
            report.close_packets[0].clone(),
            "forced-encrypted-close",
            events,
        )
        .await?;
        let down = self.peer.ingest(0, &close, self.now()?);
        if down.overflowed() != 0
            || down.len() != 1
            || !matches!(down.iter().next(), Some(Action::LinkDown { link_id }) if *link_id == old_link)
            || self.peer.has_link(old_link)
            || self.peer.transfer_active(old_link)
            || self.peer.link_count() != 0
        {
            return Err("peer did not observe one LinkDown and clear its resource".into());
        }
        events.push(
            json!({"kind":"forced_peer_link_down","old_link":format!("{old_link:?}"),
            "link_down_count":1,"peer_links":self.peer.link_count(),"peer_resources":0}),
        );

        let opening = self
            .dut
            .open_link(self.peer.destination(), 0, &random()?, self.now()?)
            .ok_or("retained DUT identity could not reopen its known peer")?;
        let request = wire(
            dut_radio,
            peer_radio,
            send_packet(&opening)?,
            "forced-fresh-request",
            events,
        )
        .await?;
        let accepted = self.peer.ingest(0, &request, self.now()?);
        if accepted.overflowed() != 0 || self.peer.link_count() != 1 {
            return Err("peer did not establish exactly one fresh link".into());
        }
        let proof = wire(
            peer_radio,
            dut_radio,
            send_packet(&accepted)?,
            "forced-fresh-proof",
            events,
        )
        .await?;
        let established = self.dut.ingest(0, &proof, self.now()?);
        let new_link = established
            .iter()
            .find_map(|action| match action {
                Action::LinkUp { link_id } => Some(*link_id),
                _ => None,
            })
            .ok_or("DUT did not establish fresh link after interruption")?;
        if established.overflowed() != 0
            || new_link == old_link
            || self.dut.link_count() != 1
            || self.peer.link_count() != 1
            || !self.dut.has_link(new_link)
            || !self.peer.has_link(new_link)
        {
            return Err("fresh link identity or count was not exact".into());
        }
        self.link = new_link;
        events.push(
            json!({"kind":"forced_fresh_link","old_link":format!("{old_link:?}"),
            "new_link":format!("{new_link:?}"),"dut_links":self.dut.link_count(),
            "peer_links":self.peer.link_count(),"same_nodes":true}),
        );
        self.exchange(dut_radio, peer_radio, "after-forced-interruption", events)
            .await
    }
}
