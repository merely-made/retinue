//! Trace the Retinue Resource state machines directly over two Tulle
//! direct-PHY radios.
//!
//! This bypasses `Endpoint` while keeping the real Link, Resource codec, and
//! RF packets. It is a bench diagnostic for separating Resource/HMU behavior
//! from Endpoint routing and session registration.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::identity::PrivateIdentity;
use retinue::link::{LinkMode, LinkTrailer, PendingLink, accept};
use retinue::packet::Packet;
use retinue::resource::RANDOM_HASH_LEN;
use retinue::resource_transfer::{ResourceReceiver, ResourceSender};
use retinue::token::IV_LEN;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

fn profile() -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz: 250_000,
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

fn link_pair() -> (retinue::link::Link, retinue::link::Link) {
    let publisher = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 255,
    };
    let destination = DestinationName::new("retinue", ["direct-phy-resource-trace"])
        .destination_hash(publisher.public());
    let (pending, request) =
        PendingLink::open(destination, *publisher.public(), &[0x43; 64], trailer);
    let (publisher_link, proof) =
        accept(&request, &publisher, &[0x44; 64], trailer).expect("accept traced link");
    let receiver_link = pending.prove(&proof).expect("prove traced link");
    (publisher_link, receiver_link)
}

fn iv_gen() -> impl FnMut() -> [u8; IV_LEN] {
    let mut sequence = 0_u64;
    move || {
        sequence += 1;
        let mut iv = [0_u8; IV_LEN];
        iv[..8].copy_from_slice(&sequence.to_le_bytes());
        iv
    }
}

async fn send(
    side: &str,
    radio: &mut DirectPhySerialLink,
    packet: Packet,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{side} tx context={} bytes={}",
        packet.context,
        packet.encoded_len()
    );
    radio.send(packet.encode()).await?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let receiver_port = args.next().unwrap_or_else(|| "COM6".into());
    let publisher_port = args.next().unwrap_or_else(|| "COM10".into());
    let input_path = args
        .next()
        .ok_or("usage: direct_phy_resource_trace RECEIVER_PORT PUBLISHER_PORT INPUT [TIMEOUT_S]")?;
    let timeout_secs = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(120);
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    let input = std::fs::read(input_path)?;

    let config = DirectPhySerialConfig {
        online_timeout: Duration::from_secs(10),
        transmit_timeout: Duration::from_secs(10),
        ..DirectPhySerialConfig::default()
    };
    let mut receiver_radio = DirectPhySerialLink::open(
        &receiver_port,
        profile(),
        AirtimeBudget::new(60_000, 60_000),
        config.clone(),
    )?;
    let mut publisher_radio = DirectPhySerialLink::open(
        &publisher_port,
        profile(),
        AirtimeBudget::new(60_000, 60_000),
        config,
    )?;
    tokio::try_join!(receiver_radio.wait_online(), publisher_radio.wait_online())?;
    println!("radios online: {receiver_port}=receiver, {publisher_port}=publisher");

    let (publisher_link, receiver_link) = link_pair();
    let mut iv = iv_gen();
    let mut random_hash = [0_u8; RANDOM_HASH_LEN];
    random_hash.copy_from_slice(&[0x51, 0x52, 0x53, 0x54]);
    let mut publisher = ResourceSender::publish(publisher_link, &input, random_hash, &iv());
    let mut receiver = ResourceReceiver::with_request_window(receiver_link, 1);
    send(
        "publisher",
        &mut publisher_radio,
        publisher.advertisement(&iv()),
    )
    .await?;

    let transfer = async {
        let mut retry = tokio::time::interval(Duration::from_secs(3));
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        retry.tick().await;
        loop {
            tokio::select! {
                received = receiver_radio.recv() => {
                    let received = match received {
                        Some(received) => received,
                        None => return Err("receiver radio closed".into()),
                    };
                    let packet = Packet::decode(&received.frame)?;
                    println!(
                        "receiver rx context={} bytes={} rssi={} snr={:.1}",
                        packet.context,
                        received.frame.len(),
                        received.rssi_dbm,
                        received.snr_db,
                    );
                    for outbound in receiver.on_packet(&packet, &mut iv) {
                        send("receiver", &mut receiver_radio, outbound).await?;
                    }
                }
                received = publisher_radio.recv() => {
                    let received = match received {
                        Some(received) => received,
                        None => return Err("publisher radio closed".into()),
                    };
                    let packet = Packet::decode(&received.frame)?;
                    println!(
                        "publisher rx context={} bytes={} rssi={} snr={:.1}",
                        packet.context,
                        received.frame.len(),
                        received.rssi_dbm,
                        received.snr_db,
                    );
                    for outbound in publisher.on_packet(&packet, &mut iv) {
                        send("publisher", &mut publisher_radio, outbound).await?;
                    }
                }
                _ = retry.tick() => {
                    if !publisher.has_started() {
                        send(
                            "publisher retry",
                            &mut publisher_radio,
                            publisher.advertisement(&iv()),
                        )
                        .await?;
                    }
                    for outbound in receiver.retransmit(&mut iv) {
                        send("receiver retry", &mut receiver_radio, outbound).await?;
                    }
                }
            }
            if publisher.is_done() && receiver.is_complete() {
                return Ok::<_, Box<dyn std::error::Error>>(());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(timeout_secs), transfer)
        .await
        .map_err(|_| "traced Resource transfer timed out")??;

    if receiver.data() != Some(input.as_slice()) {
        return Err("traced Resource payload was not byte-exact".into());
    }
    println!(
        "RETINUE DIRECT-PHY RESOURCE TRACE PASSED ({} bytes)",
        input.len()
    );
    Ok(())
}
