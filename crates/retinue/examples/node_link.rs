//! Gate N4's receipt: hear a board's announce and establish a link with it, over real RF.
//!
//! The desktop half of `retinue-small`'s first native conversation. The board is a T114
//! running its node channel — announcing on its own clock, answering link requests from
//! board-local state. This harness is desktop Retinue reaching it through a direct-PHY
//! modem, exactly as the plan's target architecture draws it:
//!
//! ```text
//! desktop Endpoint ── V4 modem ── RF ── T114 node channel
//! ```
//!
//! One run is one whole loop: wait for the board's announce, resolve its identity from it,
//! open a link, and await the proof. The caller reboots the board between runs, so every
//! run is a full boot-to-link pass rather than a warm retry — and because the board's link
//! table does not yet expire stale links (that is N5's loss-survival work), a fresh boot is
//! also what frees the slots.
//!
//! ```text
//! node_link MODEM_PORT [RUNS] [TIMEOUT_S]
//! ```
//!
//! Prints one line per run, `RUN n PASS dest=.. link=.. in a.b s` or `RUN n FAIL reason`,
//! and a final counted line for the receipt.

use core::future::Future;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::radio_io::PacketRadio;

/// A radio that narrates. Wraps the real link so the run's log shows exactly which frames
/// the desktop process sent and which came back up the serial from the modem — the
/// difference between "the proof was on the air" and "the proof reached this process".
struct LoggedRadio(DirectPhySerialLink);

#[allow(clippy::manual_async_fn)]
impl PacketRadio for LoggedRadio {
    fn max_frame_len(&self) -> usize {
        self.0.max_frame_len()
    }

    fn send_frame(
        &self,
        frame: Vec<u8>,
    ) -> impl Future<Output = Result<Duration, tulle::serial::TransmitError>> + Send {
        async move {
            let head = hex::encode(&frame[..frame.len().min(20)]);
            let result = self.0.send_frame(frame).await;
            match &result {
                Ok(air) => println!("  tx ok {}ms {head}..", air.as_millis()),
                Err(e) => println!("  tx FAILED {e:?} {head}.."),
            }
            result
        }
    }

    fn recv_frame(&mut self) -> impl Future<Output = Option<tulle::link::Received>> + Send {
        async move {
            let received = self.0.recv_frame().await;
            if let Some(r) = &received {
                println!(
                    "  rx {} bytes rssi={} {}..",
                    r.frame.len(),
                    r.rssi_dbm,
                    hex::encode(&r.frame[..r.frame.len().min(20)])
                );
            }
            received
        }
    }
}

/// The T114 node channel's boot radio state, exactly. The board never applies a host
/// profile in the node channel, so the desktop's modem must come to where the board is:
/// LongFast modulation, the Meshtastic sync word, 906.875 MHz.
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let modem_port = args.next().unwrap_or_else(|| "COM6".into());
    let runs: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(1);
    let timeout_secs: u64 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(30);
    let timeout = Duration::from_secs(timeout_secs);

    // The destination the board announces: `retinue.node` under whatever identity its flash
    // holds. The name hash is knowable in advance; the destination is learned from the air.
    let expected_name = DestinationName::new("retinue", ["node"]);

    let radio = DirectPhySerialLink::open(
        &modem_port,
        board_boot_profile(),
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    let mut radio = radio;
    tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??;
    println!("modem online: {modem_port} at SF11/250kHz sync 2b");
    let radio = LoggedRadio(radio);

    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x41; 64]));
    endpoint.set_link_mtu(255);
    // The retry must clear the whole round trip, or it collides with the answer it is
    // retrying for: at SF11/250 the request is ~0.9 s of air, the board's proof ~1.4 s more,
    // and a default tuned for fast links fires the next request straight into the proof —
    // while the board, half-duplex, misses that retry because it is still transmitting.
    // Both sides lose, every time, in perfect synchrony.
    endpoint.set_link_setup_retry(Duration::from_secs(12));
    let interface = endpoint.attach_interface();
    let driver = tokio::spawn(drive(interface, radio));

    let mut passed = 0_u32;
    for run in 1..=runs {
        let started = std::time::Instant::now();
        match one_link(&endpoint, &expected_name, timeout).await {
            Ok((dest, link_id)) => {
                passed += 1;
                println!(
                    "RUN {run} PASS dest={dest} link={link_id} in {:.1}s",
                    started.elapsed().as_secs_f64()
                );
            }
            Err(reason) => println!("RUN {run} FAIL {reason}"),
        }
        // The caller reboots the board now; drain until the next announce rather than
        // racing the reboot.
    }

    driver.abort();
    println!("=== node link: {passed} of {runs} ===");
    Ok(())
}

/// One announce-to-link pass.
async fn one_link(
    endpoint: &Endpoint,
    expected_name: &DestinationName,
    timeout: Duration,
) -> Result<(String, String), String> {
    // Wait for the board's announce. Announces for other destinations (another mesh on the
    // same sync word) are not failures; keep listening until the right name or the clock.
    let deadline = tokio::time::Instant::now() + timeout;
    let announce = loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or("no announce heard")?;
        let announce = tokio::time::timeout(remaining, endpoint.next_announcement())
            .await
            .map_err(|_| "no announce heard")?
            .map_err(|e| format!("announce stream failed: {e}"))?;
        let expected = expected_name.destination_hash(&announce.identity);
        if announce.destination == expected {
            break announce;
        }
        println!("  (ignoring announce for {})", announce.destination);
    };

    // Open a link to it. `open` sends the request and resolves once the proof arrives, so
    // its return IS link establishment from the desktop's side.
    let stream = tokio::time::timeout(
        timeout,
        endpoint.open(announce.destination, announce.identity),
    )
    .await
    .map_err(|_| "link open timed out: no proof")?
    .map_err(|e| format!("link open failed: {e}"))?;

    Ok((
        announce.destination.to_string(),
        stream.link_id().to_string(),
    ))
}
