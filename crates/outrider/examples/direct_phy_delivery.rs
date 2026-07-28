//! Bidirectional stamped LXMF delivery across two Tulle direct-PHY radios.
//!
//! Each direction carries one small Data message and one 4 KiB
//! Resource-backed message. The receiver authenticates the sender and enforces
//! the cost advertised by its `lxmf.delivery` destination.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DEFAULT_MAX_MESSAGE_BYTES, DeliveryAnnounce, LxmfPayload,
    receive_direct_with_stamp_cost_and_resource_config, register_delivery,
    send_direct_stamped_with_resource_config,
};
use retinue::endpoint::{Endpoint, PayloadMode, PeerAnnounce, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

const STAMP_COST: u8 = 8;

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

fn large_content(seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..4_096)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

struct Transfer<'a> {
    label: &'static str,
    sender: &'a Endpoint,
    sender_identity: &'a PrivateIdentity,
    receiver: Arc<Endpoint>,
    peer: &'a PeerAnnounce,
    title: &'static [u8],
    content: Vec<u8>,
    expected_mode: PayloadMode,
    stamp_seed: [u8; 32],
    timeout: Duration,
    resource_config: ResourceTransferConfig,
}

async fn transfer(spec: Transfer<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let expected_title = spec.title.to_vec();
    let expected_content = spec.content.clone();
    let receiver = Arc::clone(&spec.receiver);
    let receive_task = tokio::spawn(async move {
        let accepted = receiver.accept_resource().await?;
        receive_direct_with_stamp_cost_and_resource_config(
            &receiver,
            accepted,
            DEFAULT_MAX_MESSAGE_BYTES,
            Some(STAMP_COST),
            spec.resource_config,
        )
        .await
    });

    let payload = LxmfPayload::text(1_753_603_204.5, spec.title, spec.content);
    let started = std::time::Instant::now();
    let receipt = tokio::time::timeout(
        spec.timeout,
        send_direct_stamped_with_resource_config(
            spec.sender,
            spec.sender_identity,
            spec.peer,
            &payload,
            spec.stamp_seed,
            100_000,
            spec.resource_config,
        ),
    )
    .await??;
    let received = tokio::time::timeout(spec.timeout, receive_task).await???;

    if receipt.mode != spec.expected_mode || received.mode != spec.expected_mode {
        return Err(format!(
            "{} used {:?}/{:?}, expected {:?}",
            spec.label, receipt.mode, received.mode, spec.expected_mode
        )
        .into());
    }
    if received.message.message_id != receipt.message_id
        || received.message.payload.title != expected_title
        || received.message.payload.content != expected_content
        || received.source_identity != *spec.sender_identity.public()
    {
        return Err(format!("{} did not arrive byte-exact and authenticated", spec.label).into());
    }

    println!(
        "{}: {} bytes via {:?}, authenticated, cost-{STAMP_COST} stamp passed in {:.1}s",
        spec.label,
        expected_content.len(),
        receipt.mode,
        started.elapsed().as_secs_f64(),
    );
    Ok(())
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
    let transfer_timeout = Duration::from_secs(
        args.next()
            .map(|value| value.parse::<u64>())
            .transpose()?
            .unwrap_or(180),
    );
    let resource_config = ResourceTransferConfig {
        timeout: transfer_timeout,
        retry_interval: Duration::from_secs(3),
        request_window: 1,
    };
    let operation_timeout = transfer_timeout + Duration::from_secs(60);

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
    left.set_link_mtu(255);
    right.set_link_mtu(255);

    let left_driver = tokio::spawn(drive(left.attach_interface(), left_radio));
    let right_driver = tokio::spawn(drive(right.attach_interface(), right_radio));

    register_delivery(
        &left,
        &DeliveryAnnounce {
            display_name: Some(b"Outrider RF Left".to_vec()),
            stamp_cost: Some(STAMP_COST),
        },
    )?;
    let left_announce =
        tokio::time::timeout(Duration::from_secs(20), right.next_announcement()).await??;
    register_delivery(
        &right,
        &DeliveryAnnounce {
            display_name: Some(b"Outrider RF Right".to_vec()),
            stamp_cost: Some(STAMP_COST),
        },
    )?;
    let right_announce =
        tokio::time::timeout(Duration::from_secs(20), left.next_announcement()).await??;
    if right_announce.identity != *right_identity.public()
        || left_announce.identity != *left_identity.public()
    {
        return Err("delivery discovery resolved the wrong identity".into());
    }
    println!("discovery: cost-{STAMP_COST} lxmf.delivery announces crossed RF");

    transfer(Transfer {
        label: "left to right small",
        sender: &left,
        sender_identity: &left_identity,
        receiver: Arc::clone(&right),
        peer: &right_announce,
        title: b"LEFT SMALL",
        content: b"stamped direct PHY".to_vec(),
        expected_mode: PayloadMode::Data,
        stamp_seed: [0x10; 32],
        timeout: operation_timeout,
        resource_config,
    })
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    transfer(Transfer {
        label: "right to left small",
        sender: &right,
        sender_identity: &right_identity,
        receiver: Arc::clone(&left),
        peer: &left_announce,
        title: b"RIGHT SMALL",
        content: b"stamped direct PHY".to_vec(),
        expected_mode: PayloadMode::Data,
        stamp_seed: [0x20; 32],
        timeout: operation_timeout,
        resource_config,
    })
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    transfer(Transfer {
        label: "left to right large",
        sender: &left,
        sender_identity: &left_identity,
        receiver: Arc::clone(&right),
        peer: &right_announce,
        title: b"LEFT LARGE",
        content: large_content(0x4c52_0001),
        expected_mode: PayloadMode::Resource,
        stamp_seed: [0x30; 32],
        timeout: operation_timeout,
        resource_config,
    })
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    transfer(Transfer {
        label: "right to left large",
        sender: &right,
        sender_identity: &right_identity,
        receiver: Arc::clone(&left),
        peer: &left_announce,
        title: b"RIGHT LARGE",
        content: large_content(0x524c_0002),
        expected_mode: PayloadMode::Resource,
        stamp_seed: [0x40; 32],
        timeout: operation_timeout,
        resource_config,
    })
    .await?;

    left_driver.abort();
    right_driver.abort();
    println!("OUTRIDER DIRECT-PHY DELIVERY HEADED PASSED");
    Ok(())
}
