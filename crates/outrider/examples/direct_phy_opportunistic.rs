//! Bidirectional stamped LXMF opportunistic delivery across two Tulle direct-PHY radios.
//!
//! Each message is one ratcheted Reticulum single packet. No link or Resource
//! session is established.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DEFAULT_MAX_MESSAGE_BYTES, DeliveryAnnounce, LxmfPayload, OpportunisticError,
    receive_opportunistic_with_stamp_cost, register_opportunistic, send_opportunistic_stamped,
};
use retinue::endpoint::{Endpoint, PeerAnnounce};
use retinue::identity::{KEY_LEN, PrivateIdentity};
use retinue::iface::tulle::drive;
use retinue::ratchet::{RatchetPolicy, RatchetStore};
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

const STAMP_COST: u8 = 8;

fn single_frame_len(packed_lxmf_len: usize) -> usize {
    let plaintext_len = packed_lxmf_len - outrider::DESTINATION_LEN;
    let padded_len = (plaintext_len / 16 + 1) * 16;
    retinue::packet::HEADER_MIN_LEN
        + retinue::identity::KEY_LEN
        + retinue::token::TOKEN_OVERHEAD
        + padded_len
}

fn profile(bandwidth_hz: u32) -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz,
        spreading_factor: 8,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: 0x12,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 17,
    }
}

fn ratchets(secret: u8) -> Result<RatchetStore, retinue::ratchet::RatchetError> {
    let mut store = RatchetStore::new(RatchetPolicy::default())?;
    store.rotate_if_due([secret; KEY_LEN], 0.0)?;
    Ok(store)
}

struct Transfer<'a> {
    label: &'static str,
    sender: &'a Endpoint,
    sender_identity: &'a PrivateIdentity,
    receiver: &'a Endpoint,
    peer: &'a PeerAnnounce,
    title: &'static [u8],
    content: &'static [u8],
    stamp_seed: [u8; 32],
    timeout: Duration,
}

async fn transfer(spec: Transfer<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let payload = LxmfPayload::text(1_753_603_212.5, spec.title, spec.content);
    let receipt = send_opportunistic_stamped(
        spec.sender,
        spec.sender_identity,
        spec.peer,
        &payload,
        spec.stamp_seed,
        100_000,
    )?;
    let frame_len = single_frame_len(receipt.packed.len());
    let single = tokio::time::timeout(spec.timeout, spec.receiver.accept_single()).await??;
    let received = receive_opportunistic_with_stamp_cost(
        spec.receiver,
        single,
        DEFAULT_MAX_MESSAGE_BYTES,
        Some(STAMP_COST),
    )?;

    if received.message.message_id != receipt.message_id
        || received.message.payload.title != spec.title
        || received.message.payload.content != spec.content
        || received.source_identity != *spec.sender_identity.public()
        || received.ratchet_id != receipt.ratchet_id
    {
        return Err(format!("{} did not arrive byte-exact and authenticated", spec.label).into());
    }

    println!(
        "{}: {} signed LXMF bytes, {frame_len}-byte ratcheted RF packet, cost-{STAMP_COST} stamp passed",
        spec.label,
        receipt.packed.len(),
    );
    Ok(())
}

async fn discover(
    listener: &Endpoint,
    announcer: &Endpoint,
    destination: retinue::AddressHash,
    app_data: &[u8],
    label: &str,
) -> Result<PeerAnnounce, Box<dyn std::error::Error>> {
    for attempt in 1..=4 {
        if attempt > 1 {
            announcer.announce(&outrider::delivery_name(), app_data);
            println!("{label}: re-announced after missed RF discovery");
        }
        let received = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let peer = listener.next_announcement().await?;
                if peer.destination == destination {
                    return Ok::<_, std::io::Error>(peer);
                }
            }
        })
        .await;
        match received {
            Ok(Ok(peer)) => return Ok(peer),
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {}
        }
    }
    Err(format!("{label} announce did not cross RF after four attempts").into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let left_port = args.next().unwrap_or_else(|| "COM6".into());
    let right_port = args.next().unwrap_or_else(|| "COM10".into());
    let bandwidth_hz = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(250)
        * 1_000;
    let timeout = Duration::from_secs(
        args.next()
            .map(|value| value.parse::<u64>())
            .transpose()?
            .unwrap_or(60),
    );

    let radio_config = DirectPhySerialConfig {
        online_timeout: Duration::from_secs(10),
        transmit_timeout: Duration::from_secs(10),
        ..DirectPhySerialConfig::default()
    };
    let mut left_radio = DirectPhySerialLink::open(
        &left_port,
        profile(bandwidth_hz),
        AirtimeBudget::new(60_000, 60_000),
        radio_config.clone(),
    )?;
    let mut right_radio = DirectPhySerialLink::open(
        &right_port,
        profile(bandwidth_hz),
        AirtimeBudget::new(60_000, 60_000),
        radio_config,
    )?;
    tokio::time::timeout(Duration::from_secs(15), left_radio.wait_online()).await??;
    tokio::time::timeout(Duration::from_secs(15), right_radio.wait_online()).await??;
    println!("radios online: {left_port}=left, {right_port}=right");

    let left_identity = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
    let right_identity = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let left = Arc::new(Endpoint::new(left_identity.clone()));
    let right = Arc::new(Endpoint::new(right_identity.clone()));
    let left_driver = tokio::spawn(drive(left.attach_interface(), left_radio));
    let right_driver = tokio::spawn(drive(right.attach_interface(), right_radio));

    let left_ratchets = ratchets(0x51)?;
    let right_ratchets = ratchets(0x52)?;
    let left_delivery = DeliveryAnnounce {
        display_name: Some(b"Outrider RF Left".to_vec()),
        stamp_cost: Some(STAMP_COST),
    };
    let left_app_data = left_delivery.encode()?;
    let left_destination = register_opportunistic(&left, &left_delivery, &left_ratchets)?;
    let left_announce = discover(
        &right,
        &left,
        left_destination,
        &left_app_data,
        "left delivery",
    )
    .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let right_delivery = DeliveryAnnounce {
        display_name: Some(b"Outrider RF Right".to_vec()),
        stamp_cost: Some(STAMP_COST),
    };
    let right_app_data = right_delivery.encode()?;
    let right_destination = register_opportunistic(&right, &right_delivery, &right_ratchets)?;
    let right_announce = discover(
        &left,
        &right,
        right_destination,
        &right_app_data,
        "right delivery",
    )
    .await?;
    println!("discovery: ratcheted cost-{STAMP_COST} lxmf.delivery announces crossed RF");

    let oversized = LxmfPayload::text(
        1_753_603_212.5,
        b"LEFT OPPORTUNISTIC",
        b"one signed and ratcheted direct-PHY packet",
    );
    match send_opportunistic_stamped(
        &left,
        &left_identity,
        &right_announce,
        &oversized,
        [0xEE; 32],
        100_000,
    ) {
        Err(OpportunisticError::Io(error))
            if error.kind() == std::io::ErrorKind::InvalidInput
                && error.to_string()
                    == "single packet is 291 bytes after encryption, interface frame limit is 255" =>
        {
            println!("carrier admission: refused 291-byte packet before the 255-byte RF queue");
        }
        Err(error) => return Err(format!("unexpected carrier-admission error: {error}").into()),
        Ok(_) => return Err("oversized packet incorrectly received a queue receipt".into()),
    }
    if left_driver.is_finished() || right_driver.is_finished() {
        return Err("carrier admission did not preserve both radio drivers".into());
    }

    transfer(Transfer {
        label: "left to right",
        sender: &left,
        sender_identity: &left_identity,
        receiver: &right,
        peer: &right_announce,
        title: b"L",
        content: b"left",
        stamp_seed: [0x10; 32],
        timeout,
    })
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    transfer(Transfer {
        label: "right to left",
        sender: &right,
        sender_identity: &right_identity,
        receiver: &left,
        peer: &left_announce,
        title: b"R",
        content: b"right",
        stamp_seed: [0x20; 32],
        timeout,
    })
    .await?;

    left_driver.abort();
    right_driver.abort();
    println!("OUTRIDER DIRECT-PHY OPPORTUNISTIC HEADED PASSED");
    Ok(())
}
