//! Real executor-neutral Retinue nodes retained across the bench's radio excursions.
use super::{HOME, Result};
use retinue::announce::AnnounceBlob;
use retinue::destination::DestinationName;
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::node::{Action, Actions, Node, PauseBlocked};
use retinue::packet::Packet;
use serde_json::{Value, json};
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
