//! Radio setup and exact frame transfer between the two boards.

use retinue::packet::{DestinationType, Packet, PacketType};
use sennet::node::Channel;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::{Instant, timeout};
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::personality::{
    Controller, ControllerConfig, CoveragePolicy, InstalledPersonalitySet, PersonalityId,
};
use tulle::personality_serial::{PersonalityProfile, PersonalitySerialRuntime};

use crate::{HOME, OTHER, Result};

fn profiles() -> [PersonalityProfile; 2] {
    let mut home = PhyProfile::meshtastic_long_fast(906_875_000);
    home.sync_word = 0x12;
    home.tx_power_dbm = 7;
    let mut away = home;
    away.sync_word = 0x2b;
    [
        PersonalityProfile {
            personality: HOME,
            profile: home,
        },
        PersonalityProfile {
            personality: OTHER,
            profile: away,
        },
    ]
}
pub(super) fn runtime(link: DirectPhySerialLink, pinned: bool) -> Result<PersonalitySerialRuntime> {
    let c = Controller::new(
        ControllerConfig {
            home: HOME,
            pin: pinned.then_some(HOME),
            installed: InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap(),
            coverage: CoveragePolicy::AllowGap,
            max_excursion_ms: 30_000,
            return_budget_ms: 5_000,
            max_defer_ms: 1_000,
            transition_timeout_ms: 5_000,
        },
        0,
    )
    .map_err(|e| format!("controller config: {e:?}"))?;
    Ok(PersonalitySerialRuntime::new(c, link, 0, &profiles())?)
}
pub(super) async fn open(port: &str, dtr: bool) -> Result<DirectPhySerialLink> {
    let mut link = DirectPhySerialLink::open(
        port,
        profiles()[0].profile,
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            dtr,
            rts: false,
            online_timeout: Duration::from_secs(12),
            transmit_timeout: Duration::from_secs(8),
            ..Default::default()
        },
    )?;
    timeout(Duration::from_secs(16), link.wait_online()).await??;
    Ok(link)
}

fn check_payload(frame: &[u8], expected: &[u8], channel: Option<Channel>) -> Result<Value> {
    if frame != expected {
        return Err("received frame differs".into());
    }
    if let Some(channel) = channel {
        let got = channel
            .open_text(frame)
            .map_err(|e| format!("Sennet open: {e:?}"))?
            .ok_or("not Sennet text")?;
        Ok(
            json!({"protocol":"sennet","text":got.text,"source":got.header.source,"packet_id":got.header.packet_id}),
        )
    } else {
        let got = Packet::decode(frame)?;
        if got.packet_type != PacketType::Data || got.destination_type != DestinationType::Plain {
            return Err("wrong Retinue packet".into());
        }
        Ok(json!({"protocol":"retinue-plain-data","payload":String::from_utf8(got.payload)?}))
    }
}
pub(super) async fn transfer(
    sender: &mut PersonalitySerialRuntime,
    receiver: &mut PersonalitySerialRuntime,
    personality: PersonalityId,
    frame: Vec<u8>,
    channel: Option<Channel>,
    label: &str,
    events: &mut Vec<Value>,
) -> Result<Vec<u8>> {
    let start = Instant::now();
    let airtime = sender.send(personality, frame.clone()).await?;
    events.push(json!({"kind":"tx_ack","label":label,"hex":hex::encode(&frame),"airtime_ms":airtime.as_secs_f64()*1000.0}));
    let mut unrelated = 0;
    let received = timeout(Duration::from_secs(4), async {
        loop {
            let got = receiver.recv().await?.ok_or("serial RX stopped")?;
            if got.frame == frame {
                return Ok::<_, Box<dyn std::error::Error>>(got);
            }
            unrelated += 1;
        }
    })
    .await??;
    let decoded = check_payload(&received.frame, &frame, channel)?;
    events.push(json!({"kind":"rx_exact","label":label,"hex":hex::encode(&received.frame),
        "rssi_dbm":received.rssi_dbm,"snr_db":received.snr_db,"elapsed_ms":start.elapsed().as_secs_f64()*1000.0,
        "unrelated_frames":unrelated,"decoded":decoded}));
    Ok(received.frame)
}
