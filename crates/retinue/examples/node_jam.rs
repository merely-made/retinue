//! Hold the channel busy, so a board's listen-before-talk has something to hear.
//!
//! Pressure point 2's receipt needs an occupied band. This transmits back-to-back frames
//! from a modem for a stated span, filling the air the way a chatty neighbour would. The
//! board under test should find the channel busy, back off, and say so in its counters —
//! and, crucially, should still get its traffic out once the jam ends rather than having
//! given up permanently.
//!
//! ```text
//! node_jam MODEM_PORT [SECONDS] [FRAME_BYTES]
//! ```
//!
//! Not a Reticulum peer and not pretending to be: the frames are junk, sent only to occupy
//! the channel. Deliberately at the board's own profile, because a jam on a different
//! spreading factor is a jam the board's CAD would not see.

use std::time::Duration;

use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().unwrap_or_else(|| "COM6".into());
    let seconds: u64 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(30);
    let frame_bytes: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(200);

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
        // A jam is precisely the case an airtime budget exists to prevent, so it is opened
        // wide on purpose. This tool is a bench instrument, never a shipped path.
        AirtimeBudget::new(600_000, 600_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(15),
            ..DirectPhySerialConfig::default()
        },
    )?;
    tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??;
    println!("JAMMING {port} for {seconds}s at {frame_bytes}-byte frames");

    let frame: Vec<u8> = (0..frame_bytes).map(|i| (i * 7) as u8).collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let mut sent = 0_u32;
    while tokio::time::Instant::now() < deadline {
        match radio.send(frame.clone()).await {
            Ok(_) => sent += 1,
            Err(e) => {
                println!("jam send failed: {e}");
                break;
            }
        }
    }
    println!("=== jam done: {sent} frames ===");
    Ok(())
}
