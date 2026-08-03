//! Gate N5's receipt: byte-exact payload both directions over real RF.
//!
//! One run: hear the board announce, open a resource link, publish a payload to it, and
//! fetch back what the board's loopback service returns. The payload crossed the air twice —
//! desktop to board as a resource the board reassembled, board to desktop as a resource the
//! board itself published — so one byte-exact comparison proves both directions and both
//! halves of the board's transfer machinery.
//!
//! ```text
//! node_data MODEM_PORT [RUNS] [PAYLOAD_BYTES] [TIMEOUT_S]
//! ```
//!
//! The caller reboots the board between runs, as with `node_link`, so every run is a whole
//! boot-to-exchange pass and the board's bounded tables start empty.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

/// The T114 node channel's boot radio state, exactly, as in `node_link`.
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

/// A deterministic payload that is wrong everywhere if it is wrong anywhere: no run can
/// pass by matching a stale buffer from the run before, because each run's bytes differ.
fn payload(run: u32, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| {
            let x = (i as u32).wrapping_mul(2_654_435_761).wrapping_add(run);
            (x >> 24) as u8 ^ (x as u8)
        })
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let modem_port = args.next().unwrap_or_else(|| "COM7".into());
    let runs: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(1);
    let payload_len: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(1024);
    let timeout_secs: u64 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(90);
    let timeout = Duration::from_secs(timeout_secs);

    let expected_name = DestinationName::new("retinue", ["node"]);

    let mut radio = DirectPhySerialLink::open(
        &modem_port,
        board_boot_profile(),
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??;
    println!("modem online: {modem_port} at SF11/250kHz sync 2b");

    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x41; 64]));
    endpoint.set_link_mtu(255);
    // Retries must clear the round trip at SF11, exactly as node_link found: the setup
    // retry past request+proof, the transfer retry past request+part.
    endpoint.set_link_setup_retry(Duration::from_secs(12));
    let interface = endpoint.attach_interface();
    let driver = tokio::spawn(drive(interface, radio));

    let transfer = ResourceTransferConfig {
        timeout,
        retry_interval: Duration::from_secs(10),
        request_window: 1,
    };

    let mut passed = 0_u32;
    for run in 1..=runs {
        let started = std::time::Instant::now();
        match one_exchange(
            &endpoint,
            &expected_name,
            transfer,
            run,
            payload_len,
            timeout,
        )
        .await
        {
            Ok(()) => {
                passed += 1;
                println!(
                    "RUN {run} PASS {payload_len} bytes both ways byte-exact in {:.1}s",
                    started.elapsed().as_secs_f64()
                );
            }
            Err(reason) => println!("RUN {run} FAIL {reason}"),
        }
    }

    driver.abort();
    println!("=== node data: {passed} of {runs} ===");
    Ok(())
}

/// Wait for the board's announce, matching by derived destination.
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

/// One publish-and-fetch-back pass.
async fn one_exchange(
    endpoint: &Endpoint,
    expected_name: &DestinationName,
    transfer: ResourceTransferConfig,
    run: u32,
    payload_len: usize,
    timeout: Duration,
) -> Result<(), String> {
    let announce = hear_board(endpoint, expected_name, timeout).await?;

    let mut session = tokio::time::timeout(
        timeout,
        endpoint.open_resource(announce.destination, announce.identity),
    )
    .await
    .map_err(|_| "link open timed out")?
    .map_err(|e| format!("link open failed: {e}"))?;
    session.set_config(transfer);

    let sent = payload(run, payload_len);
    tokio::time::timeout(timeout, session.publish(&sent))
        .await
        .map_err(|_| "publish timed out")?
        .map_err(|e| format!("publish failed: {e}"))?;
    println!("  to board: {} bytes proved", sent.len());

    let echoed = tokio::time::timeout(timeout, session.fetch())
        .await
        .map_err(|_| "echo fetch timed out")?
        .map_err(|e| format!("echo fetch failed: {e}"))?;
    println!("  from board: {} bytes", echoed.len());

    if echoed != sent {
        return Err(format!(
            "echo differs: sent {} bytes, got {}",
            sent.len(),
            echoed.len()
        ));
    }
    Ok(())
}
