//! Real executor-neutral Retinue nodes retained across the bench's radio excursions.
use super::{HOME, Result};
use retinue::announce::AnnounceBlob;
use retinue::destination::DestinationName;
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::PrivateIdentity;
use retinue::node::{Action, Actions, Node, PauseBlocked};
use retinue::packet::Packet;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::{Instant, timeout};
use tulle::personality::{ControllerState, PauseOutcome};
use tulle::personality_serial::PersonalitySerialRuntime;

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("session entropy: {e}"))?;
    Ok(bytes)
}
fn send_packet(actions: &Actions<8>) -> Result<Packet> {
    if actions.overflowed() != 0 {
        return Err("node action overflow".into());
    }
    let packets: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        })
        .collect();
    if packets.len() != 1 {
        return Err(format!("expected one queued packet, got {}", packets.len()).into());
    }
    Ok(packets[0].clone())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Dut,
    Peer,
}

impl Side {
    fn opposite(self) -> Self {
        match self {
            Self::Dut => Self::Peer,
            Self::Peer => Self::Dut,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Dut => "dut",
            Self::Peer => "peer",
        }
    }
}

struct QueuedPacket {
    from: Side,
    packet: Packet,
}

const RESOURCE_QUEUE_LIMIT: usize = 8;
const RESOURCE_DELIVERY_LIMIT: usize = 64;
const RESOURCE_PAUSE_HORIZON_MS: u64 = 36_000;
async fn wire(
    from: &mut PersonalitySerialRuntime,
    to: &mut PersonalitySerialRuntime,
    packet: Packet,
    label: &str,
    events: &mut Vec<Value>,
) -> Result<Packet> {
    let bytes = packet.encode();
    from.send(HOME, bytes.clone()).await?;
    events.push(json!({"kind":"session_tx_ack","label":label,"hex":hex::encode(&bytes)}));
    let start = Instant::now();
    let received = timeout(Duration::from_secs(5), async {
        loop {
            let got = to.recv().await?.ok_or("session serial RX stopped")?;
            if got.frame == bytes {
                return Ok::<_, Box<dyn std::error::Error>>(got);
            }
        }
    })
    .await??;
    events.push(json!({"kind":"session_rx_exact","label":label,"hex":hex::encode(&received.frame),
        "rssi_dbm":received.rssi_dbm,"snr_db":received.snr_db,"receive_wait_ms":start.elapsed().as_secs_f64()*1000.0}));
    Ok(Packet::decode(&received.frame)?)
}

pub struct Sessions {
    dut: Node,
    peer: Node,
    link: AddressHash,
    epoch: Instant,
}
impl Sessions {
    fn now(&self) -> Result<u64> {
        u64::try_from(self.epoch.elapsed().as_millis()).map_err(|_| "session clock overflow".into())
    }

    fn queue_actions(
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

    async fn deliver_resource_packet(
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

    pub async fn establish(
        dut_radio: &mut PersonalitySerialRuntime,
        peer_radio: &mut PersonalitySerialRuntime,
        events: &mut Vec<Value>,
    ) -> Result<Self> {
        let epoch = Instant::now();
        let name = DestinationName::new("retinue", ["murmuration-session"]).name_hash();
        let mut dut = Node::new(PrivateIdentity::from_secret_bytes(&random()?), name);
        let mut peer = Node::new(PrivateIdentity::from_secret_bytes(&random()?), name);
        let blob = AnnounceBlob::mint(
            random()?,
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        )
        .map_err(|e| format!("announce timebase: {e:?}"))?;
        let announce = wire(
            peer_radio,
            dut_radio,
            peer.announce(&blob, None),
            "session-announce",
            events,
        )
        .await?;
        let learned = dut.ingest(0, &announce, epoch.elapsed().as_millis() as u64);
        if learned.overflowed() != 0 || !dut.peers().knows(peer.destination()) {
            return Err("RF announce not admitted".into());
        }
        let opening = dut
            .open_link(peer.destination(), 0, &random()?)
            .ok_or("node refused link request")?;
        let assessment = dut.pause_assessment();
        let now = epoch.elapsed().as_millis() as u64;
        if !matches!(
            assessment.can_pause_through(now, now + 31_000),
            Err(PauseBlocked::PendingHandshakes { .. })
        ) {
            return Err("pending handshake did not block pause".into());
        }
        // The protocol assessment refuses before issuing any hardware transition.
        // A pending handshake has no trustworthy retry time to give the controller.
        if dut_radio.controller().state() != ControllerState::Home {
            return Err("busy assessment changed radio state".into());
        }
        events.push(
            json!({"kind":"session_busy_refusal","assessment":format!("{assessment:?}"),
            "scope":"pending real handshake and queued action; no radio retune"}),
        );
        let request = wire(
            dut_radio,
            peer_radio,
            send_packet(&opening)?,
            "session-request",
            events,
        )
        .await?;
        let accepted = peer.ingest(0, &request, epoch.elapsed().as_millis() as u64);
        let proof = wire(
            peer_radio,
            dut_radio,
            send_packet(&accepted)?,
            "session-proof",
            events,
        )
        .await?;
        let established = dut.ingest(0, &proof, epoch.elapsed().as_millis() as u64);
        let link = established
            .iter()
            .find_map(|a| match a {
                Action::LinkUp { link_id } => Some(*link_id),
                _ => None,
            })
            .ok_or("RF proof did not establish link")?;
        if established.overflowed() != 0
            || !peer.has_link(link)
            || dut.link_count() != 1
            || peer.link_count() != 1
        {
            return Err("link state mismatch".into());
        }
        events
            .push(json!({"kind":"session_established","link":format!("{link:?}"),"handshakes":1}));
        Ok(Self {
            dut,
            peer,
            link,
            epoch,
        })
    }
    pub fn assess_pause(&self, horizon_ms: u64, events: &mut Vec<Value>) -> Result<PauseOutcome> {
        let now = self.epoch.elapsed().as_millis() as u64;
        for (side, node) in [("dut", &self.dut), ("peer", &self.peer)] {
            let assessment = node.pause_assessment();
            assessment
                .can_pause_through(
                    now,
                    now.checked_add(horizon_ms).ok_or("pause time overflow")?,
                )
                .map_err(|e| format!("{side} cannot pause: {e:?}"))?;
            events.push(
                json!({"kind":"session_pause_ready","side":side,"now_ms":now,
                "return_bound_ms":now+horizon_ms,"assessment":format!("{assessment:?}")}),
            );
        }
        Ok(PauseOutcome::Ready)
    }

    /// Drive one real resource through the retained link before any excursion.
    ///
    /// Each node action is kept in a bounded host queue and sent through the
    /// actual HOME radio path. The controller is only observed during refusal;
    /// this method never asks it to transition while a Node owns transfer work.
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

    pub async fn exchange(
        &mut self,
        dut_radio: &mut PersonalitySerialRuntime,
        peer_radio: &mut PersonalitySerialRuntime,
        label: &str,
        events: &mut Vec<Value>,
    ) -> Result<()> {
        let now = self.epoch.elapsed().as_millis() as u64;
        for node in [&mut self.dut, &mut self.peer] {
            if !node.poll(now, 0, None).is_empty() || !node.has_link(self.link) {
                return Err("retained session expired or owes work".into());
            }
        }
        for direction in 0..2 {
            let payload = format!("{label}:{direction}:retained-link").into_bytes();
            let (from, to, from_radio, to_radio) = if direction == 0 {
                (&self.dut, &mut self.peer, &mut *dut_radio, &mut *peer_radio)
            } else {
                (&self.peer, &mut self.dut, &mut *peer_radio, &mut *dut_radio)
            };
            let queued = from
                .send(self.link, 0, &payload, &random()?)
                .ok_or("retained link missing")?;
            let received = wire(
                from_radio,
                to_radio,
                send_packet(&queued)?,
                &format!("{label}-{direction}"),
                events,
            )
            .await?;
            let actions = to.ingest(0, &received, self.epoch.elapsed().as_millis() as u64);
            if actions.overflowed()!=0 || actions.len()!=1 || !actions.iter().any(|a|matches!(a,Action::Data {link_id,payload:p} if *link_id==self.link && *p==payload)) {
                return Err("retained link did not decrypt exact application data".into());
            }
            events.push(
                json!({"kind":"session_data","label":label,"direction":direction,
                "link":format!("{:?}",self.link),"dut_links":self.dut.link_count(),"peer_links":self.peer.link_count(),"plaintext":String::from_utf8(payload)?}),
            );
        }
        Ok(())
    }
}
