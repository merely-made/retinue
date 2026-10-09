//! Driving one resource through the retained link before any excursion.

use retinue::hash::full_hash;
use retinue::node::{Action, Actions, PauseBlocked};
use serde_json::{Value, json};
use std::collections::VecDeque;
use tulle::personality::ControllerState;
use tulle::personality_serial::PersonalitySerialRuntime;

use super::{QueuedPacket, Sessions, Side, random, wire};
use crate::Result;

const RESOURCE_QUEUE_LIMIT: usize = 8;
const RESOURCE_DELIVERY_LIMIT: usize = 64;
const RESOURCE_PAUSE_HORIZON_MS: u64 = 36_000;

impl Sessions {
    pub(super) fn queue_actions(
        &self,
        from: Side,
        actions: Actions<8>,
        queue: &mut VecDeque<QueuedPacket>,
        resources: &mut Vec<(Side, Vec<u8>)>,
    ) -> Result<()> {
        if actions.overflowed() != 0 {
            return Err("resource node action overflow".into());
        }
        for action in actions {
            match action {
                Action::Send {
                    interface: 0,
                    packet,
                } => {
                    if queue.len() == RESOURCE_QUEUE_LIMIT {
                        return Err("resource action queue exceeded bound".into());
                    }
                    queue.push_back(QueuedPacket { from, packet });
                }
                Action::Send { interface, .. } => {
                    return Err(
                        format!("resource action used interface {interface}, expected 0").into(),
                    );
                }
                Action::Resource { link_id, data } if link_id == self.link => {
                    resources.push((from, data));
                }
                other => return Err(format!("unexpected resource action: {other:?}").into()),
            }
        }
        Ok(())
    }

    pub(super) async fn deliver_resource_packet(
        &mut self,
        queued: QueuedPacket,
        dut_radio: &mut PersonalitySerialRuntime,
        peer_radio: &mut PersonalitySerialRuntime,
        events: &mut Vec<Value>,
    ) -> Result<(Side, Actions<8>)> {
        let label = format!(
            "resource-{}-to-{}",
            queued.from.label(),
            queued.from.opposite().label()
        );
        match queued.from {
            Side::Dut => {
                let received = wire(dut_radio, peer_radio, queued.packet, &label, events).await?;
                let now = self.now()?;
                Ok((Side::Peer, self.peer.ingest(0, &received, now)))
            }
            Side::Peer => {
                let received = wire(peer_radio, dut_radio, queued.packet, &label, events).await?;
                let now = self.now()?;
                Ok((Side::Dut, self.dut.ingest(0, &received, now)))
            }
        }
    }

    fn assert_resource_pause_blocked(
        &self,
        side: Side,
        expected: PauseBlocked,
        dut_radio: &PersonalitySerialRuntime,
        peer_radio: &PersonalitySerialRuntime,
        stage: &str,
        events: &mut Vec<Value>,
    ) -> Result<()> {
        if dut_radio.controller().state() != ControllerState::Home
            || peer_radio.controller().state() != ControllerState::Home
        {
            return Err("resource pause refusal changed a controller state".into());
        }
        let now = self.now()?;
        let assessment = match side {
            Side::Dut => self.dut.pause_assessment(),
            Side::Peer => self.peer.pause_assessment(),
        };
        let actual = assessment.can_pause_through(
            now,
            now.checked_add(RESOURCE_PAUSE_HORIZON_MS)
                .ok_or("resource pause horizon overflow")?,
        );
        if actual != Err(expected) {
            return Err(format!("{stage} pause result was {actual:?}").into());
        }
        events.push(json!({"kind":"resource_pause_blocked","stage":stage,
            "side":side.label(),"assessment":format!("{assessment:?}"),
            "controller_home":true}));
        Ok(())
    }

    /// Drive one real resource through the retained link before any excursion.
    ///
    /// Node actions pass through a bounded host queue and the real HOME radio path. The
    /// controller is only observed, never transitioned, while a Node owns transfer work.
    pub async fn resource_before_excursion(
        &mut self,
        dut_radio: &mut PersonalitySerialRuntime,
        peer_radio: &mut PersonalitySerialRuntime,
        events: &mut Vec<Value>,
    ) -> Result<()> {
        if dut_radio.controller().state() != ControllerState::Home
            || peer_radio.controller().state() != ControllerState::Home
            || self.dut.link_count() != 1
            || self.peer.link_count() != 1
        {
            return Err("resource requires both retained nodes at home on one link".into());
        }

        // A 350-byte source becomes multiple 255-MTU-constrained resource parts
        // after token sealing, while remaining well below the board part ceiling.
        let payload = random::<350>()?.to_vec();
        let random_hash = random()?;
        let initial_iv = random()?;
        let started = self
            .dut
            .publish(
                self.link,
                0,
                &payload,
                random_hash,
                &initial_iv,
                self.now()?,
            )
            .ok_or("retained sender refused resource publication")?;
        self.assert_resource_pause_blocked(
            Side::Dut,
            PauseBlocked::ActiveResources {
                inbound: 0,
                outbound: 1,
            },
            dut_radio,
            peer_radio,
            "outbound-before-advertisement",
            events,
        )?;

        let mut queue = VecDeque::new();
        let mut resources = Vec::new();
        self.queue_actions(Side::Dut, started, &mut queue, &mut resources)?;
        let advertisement = queue.pop_front().ok_or("resource advertisement missing")?;
        if !matches!(advertisement.from, Side::Dut) {
            return Err("resource advertisement direction changed".into());
        }
        let (recipient, replies) = self
            .deliver_resource_packet(advertisement, dut_radio, peer_radio, events)
            .await?;
        if !matches!(recipient, Side::Peer) {
            return Err("resource advertisement reached wrong node".into());
        }
        self.queue_actions(recipient, replies, &mut queue, &mut resources)?;
        self.assert_resource_pause_blocked(
            Side::Peer,
            PauseBlocked::ActiveResources {
                inbound: 1,
                outbound: 0,
            },
            dut_radio,
            peer_radio,
            "inbound-after-advertisement",
            events,
        )?;

        let mut proof_queued = false;
        let mut resource_parts_sent = 0_usize;
        let mut deliveries = 1_usize; // The advertisement was delivered above.
        while let Some(next) = queue.pop_front() {
            if matches!(next.from, Side::Peer)
                && next.packet.context == retinue::link::CTX_RESOURCE_PRF
            {
                queue.push_front(next);
                proof_queued = true;
                break;
            }
            if matches!(next.from, Side::Dut) && next.packet.context == retinue::link::CTX_RESOURCE
            {
                resource_parts_sent += 1;
            }
            if deliveries == RESOURCE_DELIVERY_LIMIT {
                return Err("resource delivery pump exceeded bound".into());
            }
            let (recipient, replies) = self
                .deliver_resource_packet(next, dut_radio, peer_radio, events)
                .await?;
            deliveries += 1;
            self.queue_actions(recipient, replies, &mut queue, &mut resources)?;
        }
        if !proof_queued {
            return Err("resource receiver never queued its proof".into());
        }
        if resource_parts_sent < 2 {
            return Err("resource did not exercise multiple MTU-constrained parts".into());
        }
        if resources.as_slice() != [(Side::Peer, payload.clone())] {
            return Err("resource receiver did not report one exact payload before proof".into());
        }
        self.assert_resource_pause_blocked(
            Side::Dut,
            PauseBlocked::ActiveResources {
                inbound: 0,
                outbound: 1,
            },
            dut_radio,
            peer_radio,
            "sender-before-receiver-proof",
            events,
        )?;
        events.push(json!({"kind":"resource_received_before_proof","link":format!("{:?}",self.link),
            "size":payload.len(),"digest":hex::encode(full_hash(&payload)),"resource_parts":resource_parts_sent,
            "queued_packets":queue.len()}));

        let proof = queue
            .pop_front()
            .ok_or("queued resource proof disappeared")?;
        if deliveries == RESOURCE_DELIVERY_LIMIT {
            return Err("resource delivery pump exceeded bound before proof".into());
        }
        let (recipient, replies) = self
            .deliver_resource_packet(proof, dut_radio, peer_radio, events)
            .await?;
        deliveries += 1;
        self.queue_actions(recipient, replies, &mut queue, &mut resources)?;
        if !queue.is_empty() {
            return Err("resource transfer left queued radio actions after proof".into());
        }
        if self.dut.transfer_active(self.link) || self.peer.transfer_active(self.link) {
            return Err("resource transfer remained active after proof".into());
        }
        if resources.as_slice() != [(Side::Peer, payload.clone())] {
            return Err("resource completion changed after proof delivery".into());
        }
        self.assess_pause(RESOURCE_PAUSE_HORIZON_MS, events)?;
        if self.dut.link_count() != 1
            || self.peer.link_count() != 1
            || !self.dut.has_link(self.link)
            || !self.peer.has_link(self.link)
        {
            return Err("resource drain did not retain exactly one link per node".into());
        }
        events.push(json!({"kind":"resource_drained","link":format!("{:?}",self.link),
            "size":payload.len(),"digest":hex::encode(full_hash(&payload)),"dut_links":self.dut.link_count(),
            "peer_links":self.peer.link_count(),"deliveries":deliveries,
            "pause_horizon_ms":RESOURCE_PAUSE_HORIZON_MS}));
        Ok(())
    }
}
