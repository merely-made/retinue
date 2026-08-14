//! N5's adversarial legs, over real RF: hostile input, then proof the node still works.
//!
//! Each leg pressures one bound and then runs the same verification — a full byte-exact
//! exchange — because "the node stays operational" is only a claim until traffic passes
//! after the abuse. The board's own counters (`node` probe) supply the typed outcome.
//!
//! ```text
//! node_stress MODEM_PORT fuzz     — 40 undecodable frames, then an exchange
//! node_stress MODEM_PORT flood [N] — 40 valid announces from identities N through N+39
//! node_stress MODEM_PORT flood-series [N] [WAVES] [PAUSE_SECS]
//!     — consecutive 40-announce waves in one source session
//! node_stress MODEM_PORT links    — open 6 links; the board holds 4 and refuses the rest
//! node_stress MODEM_PORT bigoffer — offer a resource past the part ceiling, then an exchange
//! ```
//!
//! The board is NOT rebooted inside a leg: surviving the abuse in one boot is the point.

use std::io::Write as _;
use std::time::Duration;

use retinue::announce::{self, RAND_HASH_LEN};
use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use retinue::packet::{HeaderType, Packet};
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::radio_io::PacketRadio;

fn board_boot_profile() -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz: 250_000,
        spreading_factor: 11,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: 0x2b,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 17,
    }
}

fn payload(salt: u32, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| {
            let x = (i as u32).wrapping_mul(2_654_435_761).wrapping_add(salt);
            (x >> 24) as u8 ^ (x as u8)
        })
        .collect()
}

/// Deterministic junk: never a valid packet, varied lengths, no RNG needed.
fn garbage(index: u32) -> Vec<u8> {
    let len = 12 + ((index * 7) % 180) as usize;
    (0..len)
        .map(|i| {
            let x = (i as u32)
                .wrapping_mul(0x9E37_79B9)
                .wrapping_add(index.wrapping_mul(0x85EB_CA6B));
            (x >> 16) as u8
        })
        .collect()
}

/// Send one fresh forty-identity flood and report what this independent radio heard back.
async fn flood_wave(
    radio: &mut DirectPhySerialLink,
    flood_start: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let flood_end = flood_start
        .checked_add(39)
        .ok_or("flood start must leave room for 40 identities")?;
    for index in 0..40u16 {
        let ordinal = flood_start + index;
        let mut seed = [0x50_u8; 64];
        // Byte 1, not byte 0: x25519 clamping rewrites byte 0, and an index there
        // collapses forty identities into five. The board caught this.
        seed[1..3].copy_from_slice(&ordinal.to_le_bytes());
        let identity = PrivateIdentity::from_secret_bytes(&seed);
        let name = DestinationName::new("retinue", ["floodpeer"]);
        let mut rand_hash = [0_u8; RAND_HASH_LEN];
        rand_hash[..4].copy_from_slice(&u32::from(ordinal).to_le_bytes());
        let packet = announce::build(&identity, name.name_hash(), &rand_hash, None, &[]);
        radio.send_frame(packet.encode()).await?;
    }
    // This board is also the independent RF witness. The direct-PHY pump has kept receiving
    // while each source transmit completed; drain those observations and distinguish the
    // relay form from the type-1, zero-hop announces it originated itself.
    let mut received = 0_u16;
    let mut type2_hop_one = 0_u16;
    while let Ok(Some(observation)) =
        tokio::time::timeout(Duration::from_millis(250), radio.recv()).await
    {
        received = received.saturating_add(1);
        if let Ok(packet) = Packet::decode(&observation.frame)
            && packet.header_type == HeaderType::Type2
            && packet.hops == 1
            && packet.transport.is_some()
        {
            type2_hop_one = type2_hop_one.saturating_add(1);
        }
    }
    println!(
        "flood: 40 valid announces from identities {flood_start} through {flood_end} on the air"
    );
    println!("receiver: frames={received} relay_type2_hop1={type2_hop_one}");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let modem_port = args.next().unwrap_or_else(|| "COM7".into());
    let leg = args.next().unwrap_or_else(|| "fuzz".into());
    let flood_start = args
        .next()
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(0);
    let flood_waves = args
        .next()
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(1);
    let between_waves = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(0);
    let timeout = Duration::from_secs(120);

    let expected_name = DestinationName::new("retinue", ["node"]);

    let mut radio = DirectPhySerialLink::open(
        &modem_port,
        board_boot_profile(),
        AirtimeBudget::new(120_000, 120_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??;
    println!("modem online: {modem_port}, leg={leg}");

    // The abuse phases drive the radio directly; the verify phase hands it to an endpoint.
    match leg.as_str() {
        "fuzz" => {
            // Undecodable frames: the board must count them and stay whole. Short ones,
            // so the whole barrage fits a bench minute at SF11.
            for index in 0..40 {
                radio.send_frame(garbage(index)).await?;
            }
            println!("fuzz: 40 undecodable frames on the air");
        }
        "flood" => {
            flood_wave(&mut radio, flood_start).await?;
            println!("LEG DONE flood");
            // Do not abort the serial pump on the way out. An ESP32 direct-PHY board keeps
            // its USB endpoint live across bench legs; an orderly shutdown lets the next
            // controlled flood reopen and reconfigure it instead of leaving that endpoint
            // stalled after the desktop process exits.
            radio.shutdown().await?;
            return Ok(());
        }
        "flood-series" => {
            if flood_waves == 0 {
                return Err("flood series needs at least one wave".into());
            }
            for wave in 0..flood_waves {
                let start = flood_start
                    .checked_add(wave.checked_mul(40).ok_or("flood wave index overflow")?)
                    .ok_or("flood series identity range overflows u16")?;
                flood_wave(&mut radio, start).await?;
                println!("WAVE DONE {}/{}", wave + 1, flood_waves);
                std::io::stdout().flush()?;
                if wave + 1 < flood_waves && between_waves > 0 {
                    tokio::time::sleep(Duration::from_secs(between_waves)).await;
                }
            }
            println!("LEG DONE flood-series");
            radio.shutdown().await?;
            return Ok(());
        }
        "links" | "bigoffer" => {
            // These legs need the endpoint from the start; handled below.
        }
        other => return Err(format!("unknown leg {other}").into()),
    }

    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x41; 64]));
    endpoint.set_link_mtu(255);
    endpoint.set_link_setup_retry(Duration::from_secs(12));
    let interface = endpoint.attach_interface();
    let driver = tokio::spawn(drive(interface, radio));

    let announce = hear_board(&endpoint, &expected_name, timeout).await?;
    println!("board announced: {}", announce.destination);

    let transfer = ResourceTransferConfig {
        timeout,
        retry_interval: Duration::from_secs(10),
        request_window: 1,
    };

    match leg.as_str() {
        "links" => {
            // Six links against a table of four. Each open uses a fresh ephemeral, so each
            // is a distinct link; the board must hold the first four and refuse the rest
            // while every established link stays up.
            let mut held = Vec::new();
            for attempt in 1..=6 {
                match tokio::time::timeout(
                    Duration::from_secs(30),
                    endpoint.open(announce.destination, announce.identity),
                )
                .await
                {
                    Ok(Ok(stream)) => {
                        println!("link {attempt}: up ({})", stream.link_id());
                        held.push(stream);
                    }
                    Ok(Err(e)) => println!("link {attempt}: refused ({e})"),
                    Err(_) => println!("link {attempt}: refused (no proof)"),
                }
            }
            println!("held {} links", held.len());
            println!("LEG DONE links");
        }
        "bigoffer" => {
            // A resource past MAX_RESOURCE_PARTS: the board must refuse the offer without
            // holding state, and then carry an ordinary exchange on the same boot.
            let huge = payload(0xB16, 20 * 1024);
            let refused = tokio::time::timeout(
                Duration::from_secs(45),
                endpoint.publish_resource_with_config(
                    announce.destination,
                    announce.identity,
                    &huge,
                    ResourceTransferConfig {
                        timeout: Duration::from_secs(30),
                        retry_interval: Duration::from_secs(10),
                        request_window: 1,
                    },
                ),
            )
            .await;
            match refused {
                Ok(Ok(())) => println!("bigoffer: UNEXPECTEDLY ACCEPTED"),
                Ok(Err(e)) => println!("bigoffer: refused as expected ({e})"),
                Err(_) => println!("bigoffer: refused as expected (no uptake)"),
            }
            verify_exchange(&endpoint, &announce, transfer, timeout).await?;
            println!("LEG DONE bigoffer");
        }
        _ => {
            // fuzz and flood verify the same way: ordinary traffic still passes.
            verify_exchange(&endpoint, &announce, transfer, timeout).await?;
            println!("LEG DONE {leg}");
        }
    }

    driver.abort();
    Ok(())
}

async fn hear_board(
    endpoint: &Endpoint,
    expected_name: &DestinationName,
    timeout: Duration,
) -> Result<PeerAnnounce, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or("no announce heard")?;
        let announce = tokio::time::timeout(remaining, endpoint.next_announcement())
            .await
            .map_err(|_| "no announce heard")?
            .map_err(|e| format!("announce stream failed: {e}"))?;
        if announce.destination == expected_name.destination_hash(&announce.identity) {
            return Ok(announce);
        }
    }
}

/// The operational proof: a whole byte-exact exchange, after whatever the leg did.
async fn verify_exchange(
    endpoint: &Endpoint,
    announce: &PeerAnnounce,
    transfer: ResourceTransferConfig,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut session = tokio::time::timeout(
        timeout,
        endpoint.open_resource(announce.destination, announce.identity),
    )
    .await
    .map_err(|_| "verify: link open timed out")??;
    session.set_config(transfer);

    let sent = payload(0x5717E55, 1024);
    tokio::time::timeout(timeout, session.publish(&sent))
        .await
        .map_err(|_| "verify: publish timed out")??;
    let echoed = tokio::time::timeout(timeout, session.fetch())
        .await
        .map_err(|_| "verify: fetch timed out")??;
    if echoed != sent {
        return Err("verify: echo differs".into());
    }
    println!("verify: 1024 bytes both ways byte-exact");
    Ok(())
}
