//! Opt-in physical packet-adapter proof; run through testing/mc3_personality_bench.py.
//! Host-driven retained Retinue Node link and Sennet state; no autonomous board scheduling.
#[path = "murmuration/session.rs"]
mod session;
use retinue::hash::AddressHash;
use retinue::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use sennet::flood::{
    FloodDecision, FloodIgnore, ManagedFlood, ManagedFloodConfig, RelayDelayWindow,
};
use sennet::node::Channel;
use sennet::packet_id::PacketIdState;
use sennet::transport::{BROADCAST_DESTINATION, ChannelKey, Header};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::{Instant, timeout};
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::personality::{
    Controller, ControllerConfig, ControllerError, ControllerEvent, ControllerState,
    CoverageEvidence, CoveragePolicy, Excursion, InstalledPersonalitySet, InterruptionPolicy,
    PauseOutcome, PersonalityId, ReturnReason, StopCapability,
};
use tulle::personality_serial::{
    PersonalityProfile, PersonalitySerialError, PersonalitySerialRuntime,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const HOME: PersonalityId = PersonalityId(1);
const OTHER: PersonalityId = PersonalityId(2);
const GAP: CoverageEvidence = CoverageEvidence { valid_until: None };
const EXCURSION_MS: u64 = 25_000;

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
fn runtime(link: DirectPhySerialLink, pinned: bool) -> Result<PersonalitySerialRuntime> {
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
async fn open(port: &str, dtr: bool) -> Result<DirectPhySerialLink> {
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
struct RetinueAdapter {
    prefix: String,
    sequence: u32,
}
impl RetinueAdapter {
    fn frame_with_length(&mut self, length: usize) -> Result<Vec<u8>> {
        let frame = self.frame("usb-boundary");
        if frame.len() > length {
            return Err("boundary target shorter than packet".into());
        }
        let mut packet = Packet::decode(&frame)?;
        packet
            .payload
            .resize(packet.payload.len() + length - frame.len(), b'x');
        let encoded = packet.encode();
        if encoded.len() != length {
            return Err("boundary packet encoding changed length".into());
        }
        Ok(encoded)
    }
    fn frame(&mut self, text: &str) -> Vec<u8> {
        self.sequence += 1;
        Packet {
            ifac: false,
            header_type: HeaderType::Type1,
            context_flag: false,
            propagation: Propagation::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: AddressHash::from_bytes([0x4d; 16]),
            context: 0,
            payload: format!("{}:{}:{text}", self.prefix, self.sequence).into_bytes(),
        }
        .encode()
    }
}
struct SennetAdapter {
    channel: Channel,
    ids: PacketIdState,
    file: File,
    sent: u32,
}
impl SennetAdapter {
    fn new(channel: Channel, source: u32, path: &Path) -> Result<Self> {
        Ok(Self {
            channel,
            ids: PacketIdState::new(source, 1),
            file: OpenOptions::new().write(true).create_new(true).open(path)?,
            sent: 0,
        })
    }
    fn frame(&mut self, text: &str) -> Result<Vec<u8>> {
        let id = self
            .ids
            .reserve()
            .map_err(|e| format!("packet reservation: {e:?}"))?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&self.ids.encode())?;
        self.file.sync_all()?;
        self.sent += 1;
        self.channel
            .seal_text(
                Header {
                    destination: BROADCAST_DESTINATION,
                    source: id.source,
                    packet_id: id.packet_id,
                    hop_limit: 0,
                    want_ack: false,
                    via_mqtt: false,
                    hop_start: 0,
                    channel_hash: self.channel.hash,
                    next_hop: 0,
                    relay_node: id.source as u8,
                },
                text,
            )
            .map_err(|e| format!("Sennet seal: {e:?}").into())
    }
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
async fn transfer(
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
async fn excursion(
    r: &mut PersonalitySerialRuntime,
    ms: u64,
    label: &str,
    sessions: &session::Sessions,
    events: &mut Vec<Value>,
) -> Result<()> {
    let readiness = sessions.assess_pause(
        ms.checked_add(11_000).ok_or("pause horizon overflow")?,
        events,
    )?;
    let t = r
        .request_excursion(
            Excursion {
                target: OTHER,
                duration_ms: ms,
                interruption: InterruptionPolicy::ResumableOnly,
            },
            GAP,
            readiness,
            StopCapability::Resumable,
        )?
        .ok_or("unexpected busy adapter")?;
    let start = Instant::now();
    let ack = r.apply_transition(t).await?;
    if !matches!(
        r.controller().state(),
        ControllerState::Away { target: OTHER, .. }
    ) {
        return Err("activation not acknowledged".into());
    }
    events.push(json!({"kind":"excursion_ack","label":label,"transition_id":t.id,
        "elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"discarded_rx":ack.discarded_rx,"state":format!("{:?}",r.controller().state())}));
    Ok(())
}
async fn restore(
    r: &mut PersonalitySerialRuntime,
    label: &str,
    finish: bool,
    sessions: &session::Sessions,
    events: &mut Vec<Value>,
) -> Result<()> {
    // Six seconds conservatively covers the runtime's five-second return budget.
    let readiness = sessions.assess_pause(6_000, events)?;
    let start = Instant::now();
    if finish {
        r.finish()?;
    }
    let t = r
        .begin_return(readiness)?
        .ok_or("unexpected return deferral")?;
    let ack = r.apply_transition(t).await?;
    if r.controller().state() != ControllerState::Home {
        return Err("home not acknowledged".into());
    }
    events.push(json!({"kind":"home_ack","label":label,"transition_id":t.id,
        "return_ms":start.elapsed().as_secs_f64()*1000.0,"discarded_rx":ack.discarded_rx}));
    let settle_ms: u64 = std::env::var("MC3_SETTLE_MS")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    if settle_ms > 500 {
        return Err("bench settle interval exceeds 500ms".into());
    }
    if settle_ms != 0 {
        tokio::time::sleep(Duration::from_millis(settle_ms)).await;
        events.push(
            json!({"kind":"post_ack_settle","label":label,"requested_ms":settle_ms,
            "return_with_settle_ms":start.elapsed().as_secs_f64()*1000.0}),
        );
    }
    Ok(())
}
async fn missed_wave(
    dut: &mut PersonalitySerialRuntime,
    peer: &mut PersonalitySerialRuntime,
    adapter: &mut RetinueAdapter,
    cycle: u32,
    events: &mut Vec<Value>,
) -> Result<()> {
    let start = Instant::now();
    let mut expected = Vec::new();
    for n in 0..2 {
        let frame = adapter.frame(&format!("absent-home-{cycle}-{n}"));
        let air = peer.send(HOME, frame.clone()).await?;
        events.push(json!({"kind":"home_wave_tx_ack","cycle":cycle,"hex":hex::encode(&frame),"airtime_ms":air.as_secs_f64()*1000.0}));
        expected.push(frame);
    }
    let until = Instant::now() + Duration::from_millis(900);
    let mut seen = Vec::new();
    let mut unrelated = 0;
    while Instant::now() < until {
        match timeout(until.saturating_duration_since(Instant::now()), dut.recv()).await {
            Ok(Ok(Some(frame))) => {
                if expected.contains(&frame.frame) {
                    seen.push(frame.frame);
                } else {
                    unrelated += 1;
                }
            }
            Ok(Ok(None)) => return Err("DUT stopped during absence measurement".into()),
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => break,
        }
    }
    let captured = expected.iter().filter(|e| seen.contains(e)).count();
    events.push(json!({"kind":"home_absence","cycle":cycle,"injected_tx_acked":2,"captured":captured,
        "not_observed":2-captured,"unrelated_frames":unrelated,"interval_ms":start.elapsed().as_secs_f64()*1000.0,
        "scope":"DUT receive opportunity absent; no independent on-air witness"}));
    if captured != 0 {
        return Err("DUT unexpectedly captured home profile while away".into());
    }
    Ok(())
}
async fn run(
    dut_port: &str,
    peer_port: &str,
    output: &Path,
    events: &mut Vec<Value>,
) -> Result<()> {
    let mut random = [0u8; 32];
    getrandom::getrandom(&mut random).map_err(|e| format!("bench entropy: {e}"))?;
    let run_id = hex::encode(&random[16..24]);
    let key: [u8; 16] = random[..16].try_into()?;
    let source_dut = u32::from_le_bytes(random[24..28].try_into()?);
    let source_peer = u32::from_le_bytes(random[28..32].try_into()?);
    if source_dut == source_peer {
        return Err("bench source collision".into());
    }
    let channel = Channel {
        hash: 8,
        key: ChannelKey::Aes128(key),
    };
    let mut ret_dut = RetinueAdapter {
        prefix: format!("{run_id}-dut"),
        sequence: 0,
    };
    let mut ret_peer = RetinueAdapter {
        prefix: format!("{run_id}-peer"),
        sequence: 0,
    };
    let mut sen_dut = SennetAdapter::new(
        channel,
        source_dut,
        &output.with_file_name(format!("{run_id}-dut.sennet-state")),
    )?;
    let mut sen_peer = SennetAdapter::new(
        channel,
        source_peer,
        &output.with_file_name(format!("{run_id}-peer.sennet-state")),
    )?;
    let mut dut = runtime(open(dut_port, false).await?, false)?;
    let mut peer = runtime(open(peer_port, true).await?, false)?;
    let mut peer_seen = ManagedFlood::new(ManagedFloodConfig {
        channel_hash: channel.hash,
        relay_node: source_peer as u8,
        seen_capacity: 16,
        delay: RelayDelayWindow::new(Duration::ZERO, Duration::ZERO)?,
    })?;
    let mut first_sennet_frame: Option<Vec<u8>> = None;
    events.push(json!({"kind":"online","run_id":run_id,"dut":dut_port,"peer":peer_port,
        "frequency_hz":906875000,"bandwidth_hz":250000,"sf":11,"power_dbm":7,"home_sync":18,"away_sync":43}));
    let mut sessions = session::Sessions::establish(&mut dut, &mut peer, events).await?;
    sessions
        .exchange(&mut dut, &mut peer, "before-excursion", events)
        .await?;
    sessions
        .resource_before_excursion(&mut dut, &mut peer, events)
        .await?;
    sessions
        .forced_resource_interruption(&mut dut, &mut peer, events)
        .await?;
    transfer(
        &mut dut,
        &mut peer,
        HOME,
        ret_dut.frame("baseline"),
        None,
        "baseline-dut",
        events,
    )
    .await?;
    transfer(
        &mut peer,
        &mut dut,
        HOME,
        ret_peer.frame("baseline"),
        None,
        "baseline-peer",
        events,
    )
    .await?;
    // The seven-byte direct-PHY RX envelope must cross full USB packet boundaries.
    for length in [57, 121] {
        transfer(
            &mut peer,
            &mut dut,
            HOME,
            ret_peer.frame_with_length(length)?,
            None,
            &format!("usb-{}-peer", length + 7),
            events,
        )
        .await?;
        transfer(
            &mut dut,
            &mut peer,
            HOME,
            ret_dut.frame_with_length(length)?,
            None,
            &format!("usb-{}-dut", length + 7),
            events,
        )
        .await?;
    }
    for cycle in 0..2 {
        excursion(
            &mut dut,
            EXCURSION_MS,
            &format!("dut-{cycle}"),
            &sessions,
            events,
        )
        .await?;
        missed_wave(&mut dut, &mut peer, &mut ret_peer, cycle, events).await?;
        excursion(
            &mut peer,
            EXCURSION_MS,
            &format!("peer-{cycle}"),
            &sessions,
            events,
        )
        .await?;
        let a = sen_dut.frame(&format!("{run_id}:dut:{cycle}"))?;
        let b = sen_peer.frame(&format!("{run_id}:peer:{cycle}"))?;
        let received_a = transfer(
            &mut dut,
            &mut peer,
            OTHER,
            a.clone(),
            Some(channel),
            "sennet-dut",
            events,
        )
        .await?;
        if !matches!(
            peer_seen.consider(&received_a)?,
            FloodDecision::Ignore(FloodIgnore::HopLimit)
        ) {
            return Err("fresh Sennet frame was not new to retained dedup state".into());
        }
        if let Some(previous) = first_sennet_frame.as_ref() {
            let received_duplicate = transfer(
                &mut dut,
                &mut peer,
                OTHER,
                previous.clone(),
                Some(channel),
                "sennet-replay-after-pause",
                events,
            )
            .await?;
            if !matches!(
                peer_seen.consider(&received_duplicate)?,
                FloodDecision::Ignore(FloodIgnore::Duplicate)
            ) {
                return Err("Sennet duplicate state did not survive pause".into());
            }
            events.push(json!({"kind":"sennet_duplicate_retained","relay_decision":"Ignore(Duplicate)",
                "scope":"same RF frame recognized by retained ManagedFlood; not a session protocol"}));
        } else {
            first_sennet_frame = Some(a);
        }
        transfer(
            &mut peer,
            &mut dut,
            OTHER,
            b,
            Some(channel),
            "sennet-peer",
            events,
        )
        .await?;
        if cycle == 1 {
            let cancellation = dut.cancel()?;
            if cancellation != ControllerEvent::ReturnRequired(ReturnReason::Cancelled) {
                return Err(format!("unexpected cancellation: {cancellation:?}").into());
            }
            events
                .push(json!({"kind":"explicit_cancellation","event":format!("{cancellation:?}")}));
        }
        restore(
            &mut dut,
            &format!("dut-{cycle}"),
            cycle != 1,
            &sessions,
            events,
        )
        .await?;
        restore(&mut peer, &format!("peer-{cycle}"), true, &sessions, events).await?;
        sessions
            .exchange(
                &mut dut,
                &mut peer,
                &format!("after-excursion-{cycle}"),
                events,
            )
            .await?;
        transfer(
            &mut dut,
            &mut peer,
            HOME,
            ret_dut.frame("returned"),
            None,
            "returned-dut",
            events,
        )
        .await?;
        transfer(
            &mut peer,
            &mut dut,
            HOME,
            ret_peer.frame("returned"),
            None,
            "returned-peer",
            events,
        )
        .await?;
    }
    excursion(&mut dut, 1_000, "deadline-dut", &sessions, events).await?;
    match dut.recv().await {
        Err(PersonalitySerialError::ReceiveDeadline) => {}
        other => return Err(format!("deadline receive: {other:?}").into()),
    }
    let event = dut.tick(GAP)?;
    if event != ControllerEvent::ReturnRequired(ReturnReason::ExcursionExpired) {
        return Err(format!("deadline event: {event:?}").into());
    }
    restore(&mut dut, "deadline-dut", false, &sessions, events).await?;
    sessions
        .exchange(&mut dut, &mut peer, "after-deadline", events)
        .await?;
    let deadline_text =
        std::env::var("MC3_DEADLINE_TEXT").unwrap_or_else(|_| "after-deadline".into());
    transfer(
        &mut peer,
        &mut dut,
        HOME,
        ret_peer.frame(&deadline_text),
        None,
        "after-deadline",
        events,
    )
    .await?;
    // Rebuild immutable policy only at acknowledged home, retaining the SAME serial session.
    let mut dut = runtime(dut.release_at_home()?, true)?;
    let refusal = dut.request_excursion(
        Excursion {
            target: OTHER,
            duration_ms: 1_000,
            interruption: InterruptionPolicy::ResumableOnly,
        },
        GAP,
        PauseOutcome::Ready,
        StopCapability::Resumable,
    );
    if !matches!(
        refusal,
        Err(PersonalitySerialError::Controller(ControllerError::Pinned(
            HOME
        )))
    ) {
        return Err(format!("pin did not refuse: {refusal:?}").into());
    }
    events.push(
        json!({"kind":"pinned_refusal","result":format!("{refusal:?}"),"same_serial_session":true}),
    );
    transfer(
        &mut peer,
        &mut dut,
        HOME,
        ret_peer.frame("pinned-home"),
        None,
        "pinned-home",
        events,
    )
    .await?;
    events.push(json!({"kind":"retained_state","retinue_dut_sequence":ret_dut.sequence,
        "retinue_peer_sequence":ret_peer.sequence,"sennet_dut_reserved":sen_dut.sent,"sennet_peer_reserved":sen_peer.sent,
        "scope":"caller-owned packet counters and channel state; no remote sessions"}));
    sessions
        .exchange(&mut dut, &mut peer, "pinned-retained-link", events)
        .await?;
    dut.release_at_home()?.shutdown().await?;
    peer.release_at_home()?.shutdown().await?;
    Ok(())
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!(
            "usage: murmuration_probe DUT_PORT PEER_PORT OUTPUT_JSON (guard with Python runner)"
        );
        std::process::exit(2);
    }
    let output = PathBuf::from(&args[2]);
    let mut events = Vec::new();
    let start = Instant::now();
    let outcome = run(&args[0], &args[1], &output, &mut events).await;
    let passed = outcome.is_ok();
    let report = json!({"passed":passed,"error":outcome.err().map(|e|e.to_string()),
        "elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"events":events,
        "scope":"host-driven retained Retinue Node link and Sennet state through board radio owner; no autonomous firmware or third-party peer survival claim"});
    if let Err(e) = std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()) {
        eprintln!("receipt write: {e}");
        std::process::exit(2);
    }
    println!("{}", report);
    if !passed {
        std::process::exit(1);
    }
}
