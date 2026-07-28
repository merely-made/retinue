//! Carry one caller-supplied byte string through a Retinue Resource over two
//! Tulle direct-PHY radios.
//!
//! The right-hand endpoint publishes; the left-hand endpoint fetches and
//! writes the received bytes to a caller-selected path. This keeps the harness
//! application-neutral while letting a sibling domain verify its own signed
//! payload after real RF carriage.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use retinue::Ifac;
use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let receiver_port = args.next().unwrap_or_else(|| "COM6".into());
    let sender_port = args.next().unwrap_or_else(|| "COM10".into());
    let input_path = args.next().map(PathBuf::from).ok_or(
        "usage: direct_phy_bytes RECEIVER_PORT SENDER_PORT INPUT OUTPUT [BW_KHZ] [TIMEOUT_S] [NETWORK_NAME PASSPHRASE]",
    )?;
    let output_path = args.next().map(PathBuf::from).ok_or(
        "usage: direct_phy_bytes RECEIVER_PORT SENDER_PORT INPUT OUTPUT [BW_KHZ] [TIMEOUT_S] [NETWORK_NAME PASSPHRASE]",
    )?;
    let bandwidth_hz = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(250)
        * 1_000;
    let timeout_secs = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(120);
    let network_name = args.next();
    let passphrase = args.next();
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    if network_name.is_some() != passphrase.is_some() {
        return Err("network name and passphrase must be supplied together".into());
    }
    let ifac = match (network_name.as_deref(), passphrase.as_deref()) {
        (Some(name), Some(phrase)) => Some(Ifac::new(Some(name), Some(phrase), 8)?),
        _ => None,
    };
    let input = std::fs::read(&input_path)?;

    let radio_config = DirectPhySerialConfig {
        online_timeout: Duration::from_secs(10),
        transmit_timeout: Duration::from_secs(10),
        ..DirectPhySerialConfig::default()
    };
    let mut receiver_radio = DirectPhySerialLink::open(
        &receiver_port,
        profile(bandwidth_hz),
        AirtimeBudget::new(60_000, 60_000),
        radio_config.clone(),
    )?;
    let mut sender_radio = DirectPhySerialLink::open(
        &sender_port,
        profile(bandwidth_hz),
        AirtimeBudget::new(60_000, 60_000),
        radio_config,
    )?;
    tokio::time::timeout(Duration::from_secs(15), receiver_radio.wait_online()).await??;
    tokio::time::timeout(Duration::from_secs(15), sender_radio.wait_online()).await??;
    println!("radios online: {receiver_port}=receiver, {sender_port}=sender");

    let receiver_id = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
    let sender_id = PrivateIdentity::from_secret_bytes(&[0x32; 64]);
    let receiver = Endpoint::new(receiver_id);
    let sender = Arc::new(Endpoint::new(sender_id.clone()));
    let logical_mtu = 255 - ifac.as_ref().map_or(0, Ifac::size);
    receiver.set_link_mtu(logical_mtu as u32);
    sender.set_link_mtu(logical_mtu as u32);

    let receiver_interface = match &ifac {
        Some(ifac) => receiver.attach_interface_with_ifac(255, ifac.clone())?,
        None => receiver.attach_interface(),
    };
    let sender_interface = match ifac {
        Some(ifac) => sender.attach_interface_with_ifac(255, ifac)?,
        None => sender.attach_interface(),
    };
    let receiver_driver = tokio::spawn(drive(receiver_interface, receiver_radio));
    let sender_driver = tokio::spawn(drive(sender_interface, sender_radio));
    println!(
        "interface: {} with logical MTU {logical_mtu}",
        if network_name.is_some() {
            "IFAC authenticated"
        } else {
            "open"
        }
    );

    let name = DestinationName::new("retinue", ["direct-phy-bytes"]);
    let destination = name.destination_hash(sender_id.public());
    sender.register_resource(name, b"caller-supplied-bytes");
    let announce =
        tokio::time::timeout(Duration::from_secs(20), receiver.next_announcement()).await??;
    if announce.destination != destination {
        return Err("received the wrong byte-carriage destination announce".into());
    }
    println!("discovery: byte-carriage destination announced over direct PHY");

    let transfer = ResourceTransferConfig {
        timeout: Duration::from_secs(timeout_secs),
        retry_interval: Duration::from_secs(3),
        request_window: 1,
    };
    let expected = input.clone();
    let publisher = tokio::spawn({
        let sender = Arc::clone(&sender);
        async move {
            let mut accepted = sender.accept_resource().await?;
            accepted.session.set_config(transfer);
            accepted.session.publish(&input).await
        }
    });
    let started = std::time::Instant::now();
    let received = receiver
        .fetch_resource_with_config(destination, *sender_id.public(), transfer)
        .await?;
    publisher.await??;
    if received != expected {
        return Err("direct-PHY Resource changed the caller-supplied bytes".into());
    }
    std::fs::write(&output_path, &received)?;

    receiver_driver.abort();
    sender_driver.abort();
    println!(
        "carriage: {} bytes passed byte-exact in {:.1}s",
        received.len(),
        started.elapsed().as_secs_f64()
    );
    println!("output: {}", output_path.display());
    println!("RETINUE DIRECT-PHY BYTES HEADED PASSED");
    Ok(())
}
