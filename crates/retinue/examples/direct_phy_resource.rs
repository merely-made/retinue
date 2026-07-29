//! Bidirectional Retinue Resource acceptance across two Tulle direct-PHY radios.
//!
//! The first link publishes from the initiating endpoint and fetches on the
//! accepting endpoint. The second link exercises the complementary endpoint
//! wrapper: the initiator fetches while the accepting endpoint publishes.
//! Supplying a network name and passphrase after the timeout argument applies
//! an eight-byte IFAC and subtracts it from the negotiated logical MTU.
//! `RETINUE_DIRECT_PHY_PREFLIGHT_SECS` and `RETINUE_DIRECT_PHY_POSTFLIGHT_SECS`
//! keep both serial handles open around RF for physical state observations.
//! `RETINUE_DIRECT_PHY_CLIENT_DTR=false` keeps DTR deasserted for a native-USB
//! client such as the V4 while the nRF CDC server retains the default.

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

fn payload(length: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let client_port = args.next().unwrap_or_else(|| "COM6".into());
    let server_port = args.next().unwrap_or_else(|| "COM10".into());
    let resource_len = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(4_096);
    let bandwidth_hz = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(250)
        * 1_000;
    let transfer_timeout_secs = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(180);
    let preflight_secs = std::env::var("RETINUE_DIRECT_PHY_PREFLIGHT_SECS")
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(0);
    let postflight_secs = std::env::var("RETINUE_DIRECT_PHY_POSTFLIGHT_SECS")
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(0);
    let client_dtr = std::env::var("RETINUE_DIRECT_PHY_CLIENT_DTR")
        .ok()
        .map(|value| value.parse::<bool>())
        .transpose()?
        .unwrap_or(true);
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

    let client_radio_config = DirectPhySerialConfig {
        dtr: client_dtr,
        online_timeout: Duration::from_secs(10),
        transmit_timeout: Duration::from_secs(10),
        ..DirectPhySerialConfig::default()
    };
    let server_radio_config = DirectPhySerialConfig {
        online_timeout: Duration::from_secs(10),
        transmit_timeout: Duration::from_secs(10),
        ..DirectPhySerialConfig::default()
    };
    let mut client_radio = DirectPhySerialLink::open(
        &client_port,
        profile(bandwidth_hz),
        AirtimeBudget::new(60_000, 60_000),
        client_radio_config,
    )?;
    let mut server_radio = DirectPhySerialLink::open(
        &server_port,
        profile(bandwidth_hz),
        AirtimeBudget::new(60_000, 60_000),
        server_radio_config,
    )?;
    tokio::time::timeout(Duration::from_secs(15), client_radio.wait_online()).await??;
    tokio::time::timeout(Duration::from_secs(15), server_radio.wait_online()).await??;
    println!("radios online: {client_port}=client, {server_port}=server");

    let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let client = Endpoint::new(client_id);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let logical_mtu = 255 - ifac.as_ref().map_or(0, Ifac::size);
    client.set_link_mtu(logical_mtu as u32);
    server.set_link_mtu(logical_mtu as u32);

    let client_interface = match &ifac {
        Some(ifac) => client.attach_interface_with_ifac(255, ifac.clone())?,
        None => client.attach_interface(),
    };
    let server_interface = match ifac {
        Some(ifac) => server.attach_interface_with_ifac(255, ifac)?,
        None => server.attach_interface(),
    };
    let client_driver = tokio::spawn(drive(client_interface, client_radio));
    let server_driver = tokio::spawn(drive(server_interface, server_radio));
    println!(
        "interface: {} with logical MTU {logical_mtu}",
        if network_name.is_some() {
            "IFAC authenticated"
        } else {
            "open"
        }
    );
    if preflight_secs > 0 {
        println!("preflight: radios held open for {preflight_secs}s before RF");
        tokio::time::sleep(Duration::from_secs(preflight_secs)).await;
    }

    let name = DestinationName::new("retinue", ["direct-phy-resource"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"COM6-COM10");
    let announce =
        tokio::time::timeout(Duration::from_secs(20), client.next_announcement()).await??;
    if announce.destination != destination {
        return Err("received the wrong resource destination announce".into());
    }
    println!("discovery: resource destination announced over direct PHY");

    let transfer = ResourceTransferConfig {
        timeout: Duration::from_secs(transfer_timeout_secs),
        retry_interval: Duration::from_secs(3),
        request_window: 1,
    };

    let outbound = payload(resource_len, 0x5252_1001);
    let expected_outbound = outbound.clone();
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await?;
            accepted.session.set_config(transfer);
            accepted.session.fetch().await
        }
    });
    let publish_started = std::time::Instant::now();
    client
        .publish_resource_with_config(destination, *server_id.public(), &outbound, transfer)
        .await?;
    let received = receiver.await??;
    if received != expected_outbound {
        return Err("client-to-server resource was not byte-exact".into());
    }
    let elapsed = publish_started.elapsed().as_secs_f64();
    println!("publish: client to server {resource_len} bytes passed in {elapsed:.1}s");

    tokio::time::sleep(Duration::from_secs(2)).await;

    let inbound = payload(resource_len, 0x5252_1002);
    let expected_inbound = inbound.clone();
    let publisher = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await?;
            accepted.session.set_config(transfer);
            accepted.session.publish(&inbound).await
        }
    });
    let fetch_started = std::time::Instant::now();
    let fetched = client
        .fetch_resource_with_config(destination, *server_id.public(), transfer)
        .await?;
    publisher.await??;
    if fetched != expected_inbound {
        return Err("server-to-client resource was not byte-exact".into());
    }
    let elapsed = fetch_started.elapsed().as_secs_f64();
    println!("fetch: server to client {resource_len} bytes passed in {elapsed:.1}s");
    if postflight_secs > 0 {
        println!("postflight: radios held open for {postflight_secs}s after RF");
        tokio::time::sleep(Duration::from_secs(postflight_secs)).await;
    }

    client_driver.abort();
    server_driver.abort();
    println!("RETINUE DIRECT-PHY RESOURCE HEADED PASSED");
    Ok(())
}
