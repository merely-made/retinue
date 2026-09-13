//! Bounded MC4c resident-protocol physical probe.
//!
//! This binary is intentionally inert until the guarded Python runner has
//! identified both boards and captured the peer's restore descriptor.  It
//! never uses a commissioned secret: setup material is drawn fresh for every
//! invocation and written only to the runner's private output directory.

#[path = "resident/retinue.rs"]
mod resident_retinue;

use getrandom::getrandom;
use radio_hand::resident_command::Command;
use radio_hand::resident_wire::{ResidentSetup, SennetKey};
use selvage::PhyProfile;
use sennet::{
    node::Channel,
    packet_id::PacketIdState,
    transport::{BROADCAST_DESTINATION, ChannelKey, Header},
};
use serde_json::json;
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{Instant, timeout},
};
use tucket::{identity::LocalIdentity, node::Node as TucketNode};
use tulle::{
    airtime::AirtimeBudget,
    direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const RETINUE: u8 = 0;
const SENNET: u8 = 1;
const TUCKET: u8 = 2;

fn profile(sync: u8) -> PhyProfile {
    // Taken from the MC4c preflight descriptor: 906.875 MHz, 250 kHz, SF7,
    // CR5, preamble 16.  Power 2 dBm keeps this acceptance excursion local.
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz: 250_000,
        spreading_factor: 7,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: sync,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 2,
    }
}
fn tucket_profile() -> PhyProfile {
    // MeshCore shares the 0x12 public sync word with the selected Retinue
    // home profile, so its 32-symbol preamble is the required distinct PHY.
    let mut value = profile(0x12);
    value.preamble_symbols = 32;
    value
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut value = [0; N];
    getrandom(&mut value).map_err(|e| format!("OS randomness unavailable: {e:?}"))?;
    Ok(value)
}
fn unix_seconds() -> Result<u32> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        .try_into()?)
}

fn setup() -> Result<ResidentSetup> {
    let sennet_key = random::<32>()?;
    let tucket_seed = random::<32>()?;
    let mut name_hash = random::<10>()?;
    if name_hash.iter().all(|b| *b == 0) {
        name_hash[0] = 1;
    }
    Ok(ResidentSetup {
        sennet_source: u32::from_le_bytes(random::<4>()?),
        sennet_channel: 2,
        sennet_key: SennetKey::Aes256(sennet_key),
        tucket_identity_seed: tucket_seed,
        retinue_name_hash: name_hash,
        profiles: [profile(0x12), profile(0x2b), tucket_profile()],
        home: RETINUE,
        pin: None,
        require_coverage: false,
        max_excursion_ms: 10_000,
        return_budget_ms: 1_500,
        max_defer_ms: 250,
        transition_timeout_ms: 2_000,
        tx_budget_ms: 700,
        frame_ttl_ms: 8_000,
        packet_lease_count: 64,
    })
}

async fn command(port: &mut serial2_tokio::SerialPort, value: Command) -> Result<()> {
    let bytes = value
        .encode()
        .ok_or("resident command failed local validation")?;
    port.write_all(&bytes).await?;
    port.flush().await?;
    Ok(())
}

async fn collect(port: &mut serial2_tokio::SerialPort, for_time: Duration) -> Result<String> {
    let until = Instant::now() + for_time;
    let mut text = Vec::new();
    let mut bytes = [0; 1024];
    while Instant::now() < until {
        match timeout(Duration::from_millis(150), port.read(&mut bytes)).await {
            Ok(Ok(n)) if n != 0 => text.extend_from_slice(&bytes[..n]),
            Ok(Err(e)) => return Err(e.into()),
            _ => {}
        }
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

async fn receive_matching<F>(
    peer: &mut DirectPhySerialLink,
    wait: Duration,
    mut test: F,
) -> Result<Vec<u8>>
where
    F: FnMut(&[u8]) -> bool,
{
    let deadline = Instant::now() + wait;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("expected RF frame timed out")?;
        let frame = timeout(remaining, peer.recv())
            .await?
            .ok_or("peer direct-PHY link closed")?
            .frame;
        if test(&frame) {
            return Ok(frame);
        }
    }
}

async fn await_home(dut: &mut serial2_tokio::SerialPort, visit_start: Instant) -> Result<String> {
    // Host silence is intentional during this period.  Status is queried only
    // after the configured bounded excursion has elapsed.
    tokio::time::sleep_until(visit_start + Duration::from_millis(12_500)).await;
    command(dut, Command::Status).await?;
    let status = collect(dut, Duration::from_millis(500)).await?;
    if !status.contains("state=Home") || !status.contains("instance: PersonalityId(0)") {
        return Err(format!("automatic home return absent: {status:?}").into());
    }
    Ok(status)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: resident_probe DUT_PORT PEER_PORT OUTPUT_JSON");
        std::process::exit(2);
    }
    let output = PathBuf::from(&args[2]);
    let started = Instant::now();
    let mut events = Vec::<serde_json::Value>::new();
    let mut dut_reports = Vec::<String>::new();
    let outcome: Result<_> = async {
        // Reuse the same isolated material after a deliberate DUT reset.  The
        // file is private runner state, never copied into the JSON receipt.
        let material_path = output.with_extension("resident-private.bin");
        let peer_state_path = output.with_extension("peer-packet-state.bin");
        if material_path.exists() && !peer_state_path.exists() {
            return Err("saved channel material requires its saved peer packet counter".into());
        }
        let material = if material_path.exists() {
            ResidentSetup::decode(&fs::read(&material_path)?)
                .map_err(|e| format!("invalid private resident material: {e:?}"))?
        } else {
            let value = setup()?;
            fs::write(&material_path, value.encode())?;
            value
        };
        let mut peer_ids = if peer_state_path.exists() {
            PacketIdState::decode(&fs::read(&peer_state_path)?)?
        } else {
            let state = PacketIdState::new(material.sennet_source.wrapping_add(1), 1);
            let mut file = fs::File::create(&peer_state_path)?;
            std::io::Write::write_all(&mut file, &state.encode())?;
            file.sync_all()?;
            state
        };
        if peer_ids.source() != material.sennet_source.wrapping_add(1) {
            return Err("host Sennet source differs from saved counter".into());
        }
        // Arm the peer on Retinue home before setup: a new resident node is
        // announce-due immediately and the first home packet is evidence.
        let mut peer = DirectPhySerialLink::open(&args[1], profile(0x12), AirtimeBudget::new(60_000, 2_000),
            DirectPhySerialConfig { dtr: true, rts: false, online_timeout: Duration::from_secs(12), ..Default::default() })?;
        timeout(Duration::from_secs(16), peer.wait_online()).await??;
        let mut dut = serial2_tokio::SerialPort::open(&args[0], 115_200)?;
        dut.set_dtr(false)?; dut.set_rts(false)?;
        dut.write_all(&material.encode()).await?; dut.flush().await?;
        let ready = collect(&mut dut, Duration::from_secs(3)).await?;
        dut_reports.push(ready.clone());
        events.push(json!({"kind":"dut_diagnostic","raw":ready}));
        if !ready.contains("resident ready") { return Err(format!("resident setup not accepted: {ready:?}").into()); }

        let home_announce = resident_retinue::prove_home(&mut peer).await?;
        events.push(home_announce.clone());

        let key = match material.sennet_key { SennetKey::Aes128(k) => ChannelKey::Aes128(k), SennetKey::Aes256(k) => ChannelKey::Aes256(k) };
        let channel = Channel { hash: material.sennet_channel, key };
        let mut sennet_ids = Vec::new(); let mut sennet_visits = Vec::new();
        let mut retained_duplicate: Option<Vec<u8>> = None;
        for visit in 0..3_u32 {
            peer.reconfigure(profile(0x2b)).await?;
            let visit_start = Instant::now();
            command(&mut dut, Command::Excursion { target: SENNET, duration_ms: 10_000, allow_loss: false }).await?;
            if let Some(frame) = retained_duplicate.as_ref() {
                peer.send(frame.clone()).await?;
                let retained = collect(&mut dut, Duration::from_secs(2)).await?;
                dut_reports.push(retained.clone());
                if !retained.contains("Duplicate") { return Err(format!("Sennet dedup did not survive switch visit {visit}: {retained:?}").into()); }
            }
            let mut outbound_text = heapless::String::<232>::new();
            use core::fmt::Write;
            write!(&mut outbound_text, "mc4c-sennet-out-{visit}")?;
            command(&mut dut, Command::SennetText { destination: BROADCAST_DESTINATION, hops: 0, want_ack: false,
                text: outbound_text }).await?;
            let outbound = receive_matching(&mut peer, Duration::from_secs(4), |f| channel.open_text(f).ok().flatten().is_some()).await?;
            let opened = channel.open_text(&outbound)?.ok_or("outbound Sennet frame lost channel")?;
            if opened.header.source != material.sennet_source || opened.text != format!("mc4c-sennet-out-{visit}") {
                return Err(format!("unexpected Sennet outbound visit {visit}: source={} text={:?}", opened.header.source, opened.text).into());
            }
            sennet_ids.push(opened.header.packet_id);
            let id = peer_ids.reserve()?;
            let mut state_file = fs::File::create(&peer_state_path)?;
            std::io::Write::write_all(&mut state_file, &peer_ids.encode())?;
            state_file.sync_all()?;
            let inbound = channel.seal_text(Header { destination: BROADCAST_DESTINATION, source: id.source, packet_id: id.packet_id,
                hop_limit: 0, want_ack: false, via_mqtt: false, hop_start: 0, channel_hash: channel.hash, next_hop: 0, relay_node: 0 },
                &format!("mc4c-sennet-in-{visit}"))?;
            peer.send(inbound.clone()).await?;
            let received = collect(&mut dut, Duration::from_secs(2)).await?;
            dut_reports.push(received.clone());
            if !received.contains("SennetReceived(Text") { return Err(format!("Sennet inbound proof absent visit {visit}: {received:?}").into()); }
            peer.send(inbound).await?;
            let duplicate = collect(&mut dut, Duration::from_secs(2)).await?;
            dut_reports.push(duplicate.clone());
            if !duplicate.contains("Duplicate") { return Err(format!("Sennet duplicate retention absent visit {visit}: {duplicate:?}").into()); }
            if retained_duplicate.is_none() { retained_duplicate = Some(channel.seal_text(Header { destination: BROADCAST_DESTINATION, source: id.source, packet_id: id.packet_id,
                hop_limit: 0, want_ack: false, via_mqtt: false, hop_start: 0, channel_hash: channel.hash, next_hop: 0, relay_node: 0 },
                &format!("mc4c-sennet-in-{visit}"))?); }
            sennet_visits.push(json!({"visit":visit,"outbound_id":opened.header.packet_id,"inbound_id":id.packet_id,"duplicate":true}));
            events.push(json!({"kind":"sennet_visit","visit":visit,"outbound_frame":hex::encode(outbound),"dut_reports":[received,duplicate]}));
            peer.reconfigure(profile(0x12)).await?;
            dut_reports.push(await_home(&mut dut, visit_start).await?);
            let home_proof = resident_retinue::probe_home(&mut peer, &home_announce).await;
            let link_close = collect(&mut dut, Duration::from_secs(1)).await?;
            dut_reports.push(link_close.clone());
            events.push(home_proof?);
            if !link_close.contains("LinkDown") { return Err(format!("Retinue post-return close absent visit {visit}: {link_close:?}").into()); }
        }
        if !sennet_ids.windows(2).all(|p| p[0] < p[1]) { return Err(format!("Sennet packet IDs not strictly increasing: {sennet_ids:?}").into()); }

        let mut tucket_peer = TucketNode::new(LocalIdentity::from_seed(random::<32>()?), false);
        let mut tucket_identity = None; let mut tucket_visits = Vec::new();
        for visit in 0..3_u32 {
            peer.reconfigure(tucket_profile()).await?;
            let visit_start = Instant::now();
            command(&mut dut, Command::Excursion { target: TUCKET, duration_ms: 10_000, allow_loss: visit == 2 }).await?;
            let timestamp = unix_seconds()?.saturating_add(visit);
            command(&mut dut, Command::TucketAdvert { timestamp, data: format!("mc4c-{visit}").as_bytes().try_into()? }).await?;
            let wire = receive_matching(&mut peer, Duration::from_secs(4), |_| true).await?;
            // MeshCore packet header length is parsed by Node; send frame through a peer node to surface the advert.
            let (tucket_events, _) = tucket_peer.on_frame(&wire);
            let advertised = tucket_events.into_iter().find_map(|e| match e { tucket::node::Event::Advert { identity, .. } => Some(identity.pub_key), _ => None }).ok_or("Tucket advert did not decode peer-side")?;
            if let Some(previous) = tucket_identity { if previous != advertised { return Err("Tucket identity changed across visits".into()); } } else { tucket_identity = Some(advertised); }
            peer.send(tucket_peer.advert_frame(timestamp.saturating_add(1), b"mc4c-peer")).await?;
            let observed = collect(&mut dut, Duration::from_secs(2)).await?;
            dut_reports.push(observed.clone());
            if !observed.contains("Tucket(Advert") { return Err(format!("Tucket inbound advert absent visit {visit}: {observed:?}").into()); }
            if visit == 0 {
                let peer_hash = tucket_peer.my_hash();
                let mut outbound_text = heapless::String::<171>::new();
                use core::fmt::Write;
                write!(&mut outbound_text, "mc4c-tucket-out")?;
                command(&mut dut, Command::TucketText { to: peer_hash, timestamp: timestamp.saturating_add(2), ttl_ms: 5_000,
                    attempts: 1, flood_last: false, text: outbound_text }).await?;
                let dut_text = receive_matching(&mut peer, Duration::from_secs(4), |_| true).await?;
                let (events, replies) = tucket_peer.on_frame(&dut_text);
                let message = events.into_iter().find_map(|e| match e { tucket::node::Event::Message { message, ack, .. } => Some((message.text, ack)), _ => None })
                    .ok_or("peer did not decode DUT Tucket text")?;
                if message.0 != "mc4c-tucket-out" { return Err("DUT Tucket text differs".into()); }
                for reply in replies { peer.send(reply).await?; }
                let acked = collect(&mut dut, Duration::from_secs(2)).await?;
                dut_reports.push(acked.clone());
                if !acked.contains("TucketAcknowledged") { return Err(format!("DUT Tucket ACK absent: {acked:?}").into()); }
                let (peer_text, _) = tucket_peer.text_frame(advertised[0], timestamp.saturating_add(3), "mc4c-tucket-in")
                    .ok_or("peer could not form Tucket text after DUT advert")?;
                peer.send(peer_text).await?;
                let inbound_text = collect(&mut dut, Duration::from_secs(2)).await?;
                dut_reports.push(inbound_text.clone());
                if !inbound_text.contains("Tucket(Message") { return Err(format!("DUT Tucket inbound text absent: {inbound_text:?}").into()); }
            }
            if visit == 2 {
                command(&mut dut, Command::TucketText { to: tucket_peer.my_hash(), timestamp: timestamp.saturating_add(9),
                    ttl_ms: 30_000, attempts: 1, flood_last: false, text: "mc4c-explicit-loss".try_into()? }).await?;
                let unacked = receive_matching(&mut peer, Duration::from_secs(3), |_| true).await?;
                let (messages, _withheld_replies) = tucket_peer.on_frame(&unacked);
                if !messages.iter().any(|event| matches!(event, tucket::node::Event::Message { message, .. } if message.text == "mc4c-explicit-loss")) {
                    return Err("explicit-loss text was not received by peer".into());
                }
                events.push(json!({"kind":"tucket_ack_deliberately_withheld", "frame":hex::encode(unacked)}));
            }
            tucket_visits.push(json!({"visit":visit,"identity_prefix":hex::encode(&advertised[..8])}));
            events.push(json!({"kind":"tucket_visit","visit":visit,"outbound_frame":hex::encode(wire),"identity_public":hex::encode(advertised)}));
            peer.reconfigure(profile(0x12)).await?;
            let home_status = await_home(&mut dut, visit_start).await?;
            if visit == 2 && !home_status.contains("TucketLost") {
                return Err(format!("explicit Tucket interruption not accounted: {home_status:?}").into());
            }
            dut_reports.push(home_status);
            let home_proof = resident_retinue::probe_home(&mut peer, &home_announce).await;
            let link_close = collect(&mut dut, Duration::from_secs(1)).await?;
            dut_reports.push(link_close.clone());
            events.push(home_proof?);
            if !link_close.contains("LinkDown") { return Err(format!("Retinue post-return close absent visit {visit}: {link_close:?}").into()); }
        }
        // Keep the peer on home while the board is explicitly away. A fresh
        // home request must be missed, then a fresh request after cancellation
        // must verify against the same retained identity.
        command(&mut dut, Command::Excursion { target: SENNET, duration_ms: 10_000, allow_loss: false }).await?;
        command(&mut dut, Command::Status).await?;
        let away = collect(&mut dut, Duration::from_millis(500)).await?;
        dut_reports.push(away.clone());
        if !away.contains("state=Away") || !away.contains("instance: PersonalityId(1)") {
            return Err("home-gap probe did not enter Sennet".into());
        }
        match resident_retinue::probe_home(&mut peer, &home_announce).await {
            Err(error) if error.to_string().starts_with("resident home proof timed out;") => {
                events.push(json!({"kind":"home_request_missed_while_away", "detail":error.to_string(), "scope":"one physical home request; observed absence, no coverage claimed"}));
            }
            Err(error) => return Err(error),
            Ok(_) => return Err("home request unexpectedly succeeded during Sennet visit".into()),
        }
        command(&mut dut, Command::Cancel).await?;
        command(&mut dut, Command::Status).await?;
        let cancelled = collect(&mut dut, Duration::from_millis(500)).await?;
        dut_reports.push(cancelled.clone());
        if !cancelled.contains("state=Home") { return Err("cancel did not restore home".into()); }
        let restored = resident_retinue::probe_home(&mut peer, &home_announce).await;
        let close = collect(&mut dut, Duration::from_secs(1)).await?;
        dut_reports.push(close.clone());
        events.push(restored?);
        if !close.contains("LinkDown") { return Err("post-gap home link did not close".into()); }
        command(&mut dut, Command::Status).await?;
        let status = collect(&mut dut, Duration::from_millis(500)).await?;
        dut_reports.push(status.clone());
        peer.shutdown().await?;
        Ok(json!({"ready":ready,"status":status,"dut_reports":dut_reports,"retinue_home":home_announce,"sennet_visits":sennet_visits,"tucket_visits":tucket_visits}))
    }.await;
    let report = json!({"passed":outcome.is_ok(),"qualified":outcome.is_ok(),"events":events,"dut_reports":dut_reports,
        "error":outcome.as_ref().err().map(|e|e.to_string()),
        "elapsed_ms":started.elapsed().as_millis(),"result":outcome.ok(),
        "scope":"MC4c resident RF visits: signed home announce, three bidirectional Sennet visits with duplicate retention, and three stable Tucket adverts"});
    let write = fs::write(&output, serde_json::to_vec_pretty(&report).unwrap());
    if let Err(e) = write {
        eprintln!("receipt write: {e}");
        std::process::exit(2);
    }
    println!("{report}");
    if !report["passed"].as_bool().unwrap_or(false) {
        std::process::exit(1);
    }
}
