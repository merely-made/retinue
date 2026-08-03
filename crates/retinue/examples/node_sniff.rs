//! Bench diagnostic: print every raw frame a direct-PHY modem hears at the T114 node
//! channel's boot profile. No endpoint, no protocol — just the air, so a silent
//! `node_link` run can be split into "nothing arrived" versus "arrived but not decoded".

use std::time::Duration;

use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().unwrap_or_else(|| "COM6".into());
    let listen_secs: u64 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(20);

    let profile = PhyProfile {
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
    };

    let mut radio = DirectPhySerialLink::open(
        &port,
        profile,
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??;
    println!("SNIFFING {port} for {listen_secs}s");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(listen_secs);
    let mut frames = 0_u32;
    loop {
        let remaining = match deadline.checked_duration_since(tokio::time::Instant::now()) {
            Some(r) => r,
            None => break,
        };
        match tokio::time::timeout(remaining, radio.recv()).await {
            Ok(Some(received)) => {
                frames += 1;
                let identity = match retinue::packet::Packet::decode(&received.frame) {
                    Ok(packet) => format!(
                        "{:?} dest={} ctx={:02x}",
                        packet.packet_type, packet.destination, packet.context
                    ),
                    Err(e) => format!("undecoded: {e:?}"),
                };
                println!(
                    "FRAME {} bytes rssi={} snr={} {identity}",
                    received.frame.len(),
                    received.rssi_dbm,
                    received.snr_db,
                );
                println!("  {}", hex::encode(&received.frame));
            }
            Ok(None) => {
                println!("link closed");
                break;
            }
            Err(_) => break,
        }
    }
    println!("=== {frames} frames heard ===");
    Ok(())
}
