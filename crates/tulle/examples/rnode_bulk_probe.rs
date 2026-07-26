//! Bulk-TX probe: one RNode floods sequence-tagged frames at another, which
//! counts arrivals and reports loss, gaps, and stalls.
//!
//! Built to close the 2026-07-22 finding: repeated 243-byte frames from the
//! T114 under RNode firmware sometimes stopped arriving. Both endpoints are
//! stock RNodes, which is the pairing that produced the original observation.
//! (A direct-PHY listener cannot serve here: the two firmwares do not share a
//! demodulation configuration, see 2026-07-25_rnode_direct_phy_rf_opacity.md.)
//!
//! Usage:
//! `cargo run --features serial-async --example rnode_bulk_probe -- COM5 COM7 [count] [frame_len] [bw_khz]`

use std::time::{Duration, Instant};

use tulle::airtime::AirtimeBudget;
use tulle::lora::{CodingRate, LoRaParams};
use tulle::serial::{RNodeSerialLink, SerialPumpConfig};

/// Silence long enough to call the sender stalled rather than slow.
const STALL_TIMEOUT: Duration = Duration::from_secs(20);

fn fill(buffer: &mut [u8], seed: u32) {
    let mut state = seed | 1;
    for byte in buffer.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let sender_port = args.next().unwrap_or_else(|| "COM5".into());
    let receiver_port = args.next().unwrap_or_else(|| "COM7".into());
    let count: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(200);
    let frame_len: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(243);
    let bandwidth_hz: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(125) * 1_000;
    let pace_ms: u64 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(180);
    let tx_power_dbm: u8 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(7);
    // CRC off makes the radio deliver corrupted frames instead of discarding
    // them, which separates "arrived damaged" from "never arrived".
    let crc = args.next().map(|v| v != "0").unwrap_or(true);

    let params = LoRaParams {
        spreading_factor: 8,
        bandwidth_hz,
        coding_rate: CodingRate::Cr45,
        frequency_hz: 915_000_000,
        tx_power_dbm,
        preamble_syms: 8,
        explicit_header: true,
        crc,
    };

    let mut sender = RNodeSerialLink::open(
        &sender_port,
        params,
        AirtimeBudget::new(60_000, 60_000),
        SerialPumpConfig::default(),
    )?;
    let mut receiver = RNodeSerialLink::open(
        &receiver_port,
        params,
        AirtimeBudget::new(60_000, 60_000),
        SerialPumpConfig::default(),
    )?;
    let sender_fw = tokio::time::timeout(Duration::from_secs(25), sender.wait_online()).await??;
    let receiver_fw =
        tokio::time::timeout(Duration::from_secs(25), receiver.wait_online()).await??;
    println!(
        "online: {sender_port}={sender_fw:?} sender, {receiver_port}={receiver_fw:?} receiver"
    );
    println!(
        "profile: 915 MHz, BW {} kHz, SF8, CR4/5, {tx_power_dbm} dBm, {frame_len}-byte frames, {count} of them, pace airtime+{pace_ms}ms",
        bandwidth_hz / 1000
    );

    // Smoke first: one frame must cross before a flood means anything.
    let smoke = b"tulle-bulk-smoke".to_vec();
    sender.send(smoke.clone()).await?;
    let heard = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match receiver.recv().await {
                Some(received) if received.frame == smoke => return Some(received),
                Some(_) => continue,
                None => return None,
            }
        }
    })
    .await;
    match heard {
        Ok(Some(received)) => println!(
            "smoke: one frame crossed (RSSI {} dBm, SNR {} dB)",
            received.rssi_dbm, received.snr_db
        ),
        _ => {
            println!("smoke FAILED: the link is not carrying frames at all");
            std::process::exit(2);
        }
    }

    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let listen = tokio::spawn(async move {
        let mut seen = vec![false; count as usize];
        let mut received = 0u32;
        let mut delivered = 0u32;
        let mut corrupted = 0u32;
        let mut last_arrival = Instant::now();
        let mut longest_gap = Duration::ZERO;
        let mut gap_before_seq = 0u32;
        let mut weakest_rssi = i16::MAX;
        loop {
            match tokio::time::timeout(STALL_TIMEOUT, receiver.recv()).await {
                Ok(Some(frame)) => {
                    delivered += 1;
                    let mut intact = false;
                    if frame.frame.len() >= 4 {
                        let seq = u32::from_be_bytes(frame.frame[..4].try_into().unwrap());
                        if (seq as usize) < seen.len() {
                            let mut expected = vec![0u8; frame_len.max(4)];
                            expected[..4].copy_from_slice(&seq.to_be_bytes());
                            fill(&mut expected[4..], seq.wrapping_mul(0x9e37_79b9));
                            intact = frame.frame == expected;
                            if intact && !seen[seq as usize] {
                                seen[seq as usize] = true;
                                received += 1;
                                if last_arrival.elapsed() > longest_gap {
                                    longest_gap = last_arrival.elapsed();
                                    gap_before_seq = seq;
                                }
                                last_arrival = Instant::now();
                                weakest_rssi = weakest_rssi.min(frame.rssi_dbm);
                            }
                        }
                    }
                    if !intact {
                        corrupted += 1;
                    }
                    if received == count {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break, // the stall this probe exists to catch
            }
        }
        let missing: Vec<u32> = (0..count).filter(|&i| !seen[i as usize]).collect();
        let _ = done_tx.send((
            received,
            delivered,
            corrupted,
            missing,
            longest_gap,
            gap_before_seq,
            weakest_rssi,
        ));
    });

    let started = Instant::now();
    let mut frame = vec![0u8; frame_len.max(4)];
    for seq in 0..count {
        frame[..4].copy_from_slice(&seq.to_be_bytes());
        fill(&mut frame[4..], seq.wrapping_mul(0x9e37_79b9));
        let airtime = sender.send(frame.clone()).await?;
        // One frame per airtime plus turnaround keeps the channel breathing
        // (2026-07-21 finding 2). The probe measures the radio, not the pump.
        tokio::time::sleep(airtime + Duration::from_millis(pace_ms)).await;
        if (seq + 1) % 50 == 0 {
            println!("sent {} of {count}", seq + 1);
        }
    }
    println!(
        "flood sent: {count} frames in {:.1}s",
        started.elapsed().as_secs_f64()
    );

    let (received, delivered, corrupted, missing, longest_gap, gap_before_seq, weakest_rssi) =
        done_rx.await?;
    println!(
        "radio delivered {delivered} frames: {received} intact, {corrupted} corrupted or unusable"
    );
    println!(
        "received {received} of {count}; longest gap {:.1}s (before seq {gap_before_seq}); weakest RSSI {weakest_rssi} dBm",
        longest_gap.as_secs_f64()
    );
    listen.abort();
    if missing.is_empty() {
        println!("RNODE BULK TX PROBE PASSED: no loss, no stall");
    } else {
        let head: Vec<u32> = missing.iter().copied().take(24).collect();
        println!("missing {} frames; first missing: {head:?}", missing.len());
        println!("RNODE BULK TX PROBE FAILED");
        std::process::exit(1);
    }
    Ok(())
}
