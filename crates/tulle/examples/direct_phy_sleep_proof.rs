//! Prove ESP32-S3 Light-sleep entry and SX1262 DIO1 wake without a current meter.
//!
//! The sleeping V4 has no live USB connection. This harness drives the USB-attached T114,
//! sends nonce-bearing RF challenges, and accepts only matching RF receipts containing the
//! V4's sleep-entry, blocked-idle, and receive counters.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

const FREQUENCY_HZ: u32 = 906_875_000;
const CHALLENGE_PREFIX: &[u8; 21] = b"tulle/sleep-proof/v1?";
const RECEIPT_PREFIX: &[u8; 21] = b"tulle/sleep-proof/v1!";
const CHALLENGE_LEN: usize = CHALLENGE_PREFIX.len() + 4;
const RECEIPT_LEN: usize = RECEIPT_PREFIX.len() + 28;

#[derive(Clone, Copy, Debug)]
struct Receipt {
    nonce: u32,
    sleep_entries: u32,
    wake_registrations: u32,
    received_frames: u32,
    last_sleep_us: u32,
    sleep_enabled: bool,
    reset_reason: u32,
}

fn challenge(nonce: u32) -> [u8; CHALLENGE_LEN] {
    let mut frame = [0_u8; CHALLENGE_LEN];
    frame[..CHALLENGE_PREFIX.len()].copy_from_slice(CHALLENGE_PREFIX);
    frame[CHALLENGE_PREFIX.len()..].copy_from_slice(&nonce.to_le_bytes());
    frame
}

fn receipt(frame: &[u8]) -> Option<Receipt> {
    if frame.len() != RECEIPT_LEN || !frame.starts_with(RECEIPT_PREFIX) {
        return None;
    }
    Some(Receipt {
        nonce: u32::from_le_bytes(frame[21..25].try_into().ok()?),
        sleep_entries: u32::from_le_bytes(frame[25..29].try_into().ok()?),
        wake_registrations: u32::from_le_bytes(frame[29..33].try_into().ok()?),
        received_frames: u32::from_le_bytes(frame[33..37].try_into().ok()?),
        last_sleep_us: u32::from_le_bytes(frame[37..41].try_into().ok()?),
        sleep_enabled: u32::from_le_bytes(frame[41..45].try_into().ok()?) != 0,
        reset_reason: u32::from_le_bytes(frame[45..49].try_into().ok()?),
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port = args.next().unwrap_or_else(|| "COM10".into());
    let cycles = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(100);
    let expect_sleep = match args.next().as_deref() {
        None | Some("sleep") => true,
        Some("awake") => false,
        Some(_) => {
            return Err(
                "usage: direct_phy_sleep_proof [T114_PORT] [CYCLES] [sleep|awake] [AWAKE_V4_PORT]"
                    .into(),
            );
        }
    };
    let awake_v4_port = args.next();
    if cycles == 0 {
        return Err("cycle count must be greater than zero".into());
    }
    if expect_sleep && cycles < 3 {
        return Err("sleep mode needs at least three cycles: awake control, first wake, receipt with post-wake counter".into());
    }
    if expect_sleep && awake_v4_port.is_some() {
        return Err("an awake V4 control port is valid only in awake mode".into());
    }
    if args.next().is_some() {
        return Err(
            "usage: direct_phy_sleep_proof [T114_PORT] [CYCLES] [sleep|awake] [AWAKE_V4_PORT]"
                .into(),
        );
    }

    let config = DirectPhySerialConfig {
        online_timeout: Duration::from_secs(10),
        transmit_timeout: Duration::from_secs(10),
        ..DirectPhySerialConfig::default()
    };
    let mut radio = DirectPhySerialLink::open(
        &port,
        PhyProfile::meshtastic_long_fast(FREQUENCY_HZ),
        AirtimeBudget::new(60_000, 60_000),
        config.clone(),
    )?;
    let mut awake_v4 = match awake_v4_port.as_deref() {
        Some(path) => Some(DirectPhySerialLink::open(
            path,
            PhyProfile::meshtastic_long_fast(FREQUENCY_HZ),
            AirtimeBudget::new(60_000, 60_000),
            config,
        )?),
        None => None,
    };
    match awake_v4.as_mut() {
        Some(v4) => {
            tokio::time::timeout(Duration::from_secs(15), async {
                tokio::try_join!(radio.wait_online(), v4.wait_online())
            })
            .await??;
        }
        None => tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??,
    }
    println!("radio online: {port}=T114 witness; profile=906875000/SF11/BW250/CR4-5/sync2b");
    if let Some(path) = awake_v4_port.as_deref() {
        println!("awake control online: {path}=V4 responder host");
    }

    // Let the V4 finish booting and reach its first genuinely idle receive window.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let seed = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u32;
    let mut previous_sleep_entries = 0;
    let mut previous_received_frames = 0;
    let mut first = None;
    let mut last = None;

    for cycle in 1..=cycles {
        let nonce = seed.wrapping_add(cycle);
        let challenge = challenge(nonce);
        radio.send(challenge.to_vec()).await?;
        if let Some(v4) = awake_v4.as_mut() {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let received = v4.recv().await.ok_or("awake V4 receive lane closed")?;
                    if received.frame == challenge {
                        return Ok::<_, &'static str>(());
                    }
                }
            })
            .await
            .map_err(|_| format!("cycle {cycle}: awake V4 did not report the RF challenge"))??;
        }
        let got = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let received = radio.recv().await?;
                let Some(parsed) = receipt(&received.frame) else {
                    continue;
                };
                if parsed.nonce == nonce {
                    return Some((parsed, received.rssi_dbm, received.snr_db));
                }
            }
        })
        .await
        .map_err(|_| format!("cycle {cycle}: matching RF receipt timed out"))?
        .ok_or_else(|| format!("cycle {cycle}: receive lane closed"))?;
        let (got, rssi_dbm, snr_db) = got;

        if cycles <= 10
            || cycle == 1
            || cycle == cycles
            || cycle % 25 == 0
            || got.sleep_entries == 0
        {
            println!(
                "cycle {cycle}/{cycles}: enabled={} armed={} sleep={} last={}us rx={} reset=0x{:02x} rssi={}dBm snr={:.1}dB",
                got.sleep_enabled,
                got.wake_registrations,
                got.sleep_entries,
                got.last_sleep_us,
                got.received_frames,
                got.reset_reason,
                rssi_dbm,
                snr_db
            );
        }

        if expect_sleep && cycle > 1 && got.sleep_entries <= previous_sleep_entries {
            return Err(format!(
                "cycle {cycle}: sleep counter did not advance ({} -> {})",
                previous_sleep_entries, got.sleep_entries
            )
            .into());
        }
        if cycle > 1 && got.received_frames <= previous_received_frames {
            return Err(format!(
                "cycle {cycle}: receive counter did not advance ({} -> {})",
                previous_received_frames, got.received_frames
            )
            .into());
        }

        let observation = (
            got.sleep_entries,
            got.wake_registrations,
            got.received_frames,
            rssi_dbm,
            snr_db,
        );
        first.get_or_insert(observation);
        last = Some(observation);
        previous_sleep_entries = got.sleep_entries;
        previous_received_frames = got.received_frames;

        // The next challenge must arrive after the V4 has returned to continuous receive,
        // registered DIO1 as a wake source, and entered Light-sleep. SF11 receipts occupy a
        // substantial part of the prior second, so leave a full quiet interval afterward.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
    }

    let first = first.expect("at least one cycle");
    let last = last.expect("at least one cycle");
    if expect_sleep && last.0 == 0 {
        return Err("post-wake Light-sleep counter never advanced".into());
    }
    println!(
        "counter span: sleep {}..{}; DIO arms {}..{}; rx {}..{}",
        first.0, last.0, first.1, last.1, first.2, last.2
    );
    if expect_sleep {
        println!("TULLE V4 LIGHT-SLEEP RF WAKE PROOF PASSED: {cycles}/{cycles} matching receipts");
    } else {
        println!(
            "TULLE V4 AWAKE RF RECEIPT CONTROL PASSED: {cycles}/{cycles} matching receipts; sleep={}",
            last.0
        );
    }
    if let Some(v4) = awake_v4 {
        v4.shutdown().await?;
    }
    radio.shutdown().await?;
    Ok(())
}
