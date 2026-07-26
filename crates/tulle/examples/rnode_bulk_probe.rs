//! Bulk-TX probe: a stock RNode floods sequence-tagged frames at a direct-PHY
//! listener, which counts arrivals and reports gaps and stalls.
//!
//! Built to close the 2026-07-22 finding (T114 bulk TX under RNode firmware
//! sometimes stopped arriving). The receiver is direct-PHY so the only stock
//! firmware in the path is the sender's.
//!
//! Usage:
//! `cargo run --features serial-async --example rnode_bulk_probe -- COM5 COM6 [count] [frame_len] [bw_khz] [sync_hex]`

use std::time::{Duration, Instant};

use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::lora::{CodingRate, LoRaParams};
use tulle::serial::{RNodeSerialLink, SerialPumpConfig};

fn fill(buffer: &mut [u8], seed: u32) {
    let mut state = seed | 1;
    for byte in buffer.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let rnode_port = args.next().unwrap_or_else(|| "COM5".into());
    let phy_port = args.next().unwrap_or_else(|| "COM6".into());
    let count: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(200);
    let frame_len: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(243);
    let bandwidth_hz: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(125) * 1_000;
    let sync_word = u8::from_str_radix(&args.next().unwrap_or_else(|| "12".into()), 16)?;
    let invert_iq = args.next().map(|v| v == "1").unwrap_or(false);
    let reverse_smoke = args.next().map(|v| v == "rev").unwrap_or(false);

    let params = LoRaParams {
        spreading_factor: 8,
        bandwidth_hz,
        coding_rate: CodingRate::Cr45,
        frequency_hz: 915_000_000,
        tx_power_dbm: 7,
        preamble_syms: 8,
        explicit_header: true,
        crc: true,
    };
    let profile = PhyProfile {
        frequency_hz: 915_000_000,
        bandwidth_hz,
        spreading_factor: 8,
        coding_rate_denominator: 5,
        preamble_symbols: 8,
        sync_word,
        explicit_header: true,
        crc: true,
        invert_iq,
        tx_power_dbm: 7,
    };

    let mut sender = RNodeSerialLink::open(
        &rnode_port,
        params,
        AirtimeBudget::new(60_000, 60_000),
        SerialPumpConfig::default(),
    )?;
    let mut listener = DirectPhySerialLink::open(
        &phy_port,
        profile,
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    let firmware = tokio::time::timeout(Duration::from_secs(25), sender.wait_online()).await??;
    tokio::time::timeout(Duration::from_secs(15), listener.wait_online()).await??;
    println!("online: {rnode_port}=RNode {firmware:?} sender, {phy_port}=direct-PHY listener");
    println!(
        "profile: 915 MHz, BW {} kHz, SF8, CR4/5, sync 0x{sync_word:02x}, invert_iq={invert_iq}, {frame_len}-byte frames",
        bandwidth_hz / 1000
    );

    if reverse_smoke {
        // Direction check: direct-PHY transmits, the RNode listens. If this crosses
        // while the forward smoke does not, the sync is right and the RNode's own
        // transmit path is the broken half.
        let probe = b"tulle-reverse-probe".to_vec();
        listener.send(probe.clone()).await?;
        let heard = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match sender.recv().await {
                    Some(received) if received.frame == probe => return Some(received),
                    Some(_) => continue,
                    None => return None,
                }
            }
        })
        .await;
        match heard {
            Ok(Some(received)) => {
                println!(
                    "reverse smoke: direct-PHY frame heard by RNode (RSSI {} dBm, SNR {} dB)",
                    received.rssi_dbm, received.snr_db
                );
                std::process::exit(0);
            }
            _ => {
                println!("reverse smoke FAILED at sync 0x{sync_word:02x}");
                std::process::exit(2);
            }
        }
    }

    // Sync-word smoke: one distinctive frame must cross before the flood means anything.
    let smoke = b"tulle-sync-probe".to_vec();
    sender.send(smoke.clone()).await?;
    let heard = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match listener.recv().await {
                Some(received) if received.frame == smoke => return Some(received),
                Some(_) => continue,
                None => return None,
            }
        }
    })
    .await;
    match heard {
        Ok(Some(received)) => println!(
            "smoke: RNode frame crossed to direct-PHY (RSSI {} dBm, SNR {} dB)",
            received.rssi_dbm, received.snr_db
        ),
        _ => {
            println!("smoke FAILED: no frame at sync 0x{sync_word:02x}; try another sync value");
            std::process::exit(2);
        }
    }

    // The flood. The listener runs as its own task and reports what arrived.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let listen = tokio::spawn(async move {
        let mut seen = vec![false; count as usize];
        let mut received = 0u32;
        let mut last_arrival = Instant::now();
        let mut longest_gap = Duration::ZERO;
        let mut weakest_rssi = i16::MAX;
        loop {
            match tokio::time::timeout(Duration::from_secs(20), listener.recv()).await {
                Ok(Some(frame)) => {
                    if frame.frame.len() >= 4 {
                        let seq = u32::from_be_bytes(frame.frame[..4].try_into().unwrap());
                        if (seq as usize) < seen.len() && !seen[seq as usize] {
                            seen[seq as usize] = true;
                            received += 1;
                            longest_gap = longest_gap.max(last_arrival.elapsed());
                            last_arrival = Instant::now();
                            weakest_rssi = weakest_rssi.min(frame.rssi_dbm);
                        }
                    }
                    if received == count {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break, // 20 s of silence: the stall the probe exists to catch.
            }
        }
        let missing: Vec<u32> = (0..count).filter(|&i| !seen[i as usize]).collect();
        let _ = done_tx.send((received, missing, longest_gap, weakest_rssi));
    });

    let started = Instant::now();
    let mut frame = vec![0u8; frame_len.max(4)];
    for seq in 0..count {
        frame[..4].copy_from_slice(&seq.to_be_bytes());
        fill(&mut frame[4..], seq.wrapping_mul(0x9e37_79b9));
        let airtime = sender.send(frame.clone()).await?;
        // One frame per airtime plus turnaround keeps the channel breathing
        // (2026-07-21 finding 2); the probe measures the sender, not the pump.
        tokio::time::sleep(airtime + Duration::from_millis(180)).await;
        if (seq + 1) % 50 == 0 {
            println!("sent {} of {count}", seq + 1);
        }
    }
    println!(
        "flood sent: {count} frames in {:.1}s",
        started.elapsed().as_secs_f64()
    );

    let (received, missing, longest_gap, weakest_rssi) = done_rx.await?;
    println!(
        "received {received} of {count}; longest inter-arrival gap {:.1}s; weakest RSSI {weakest_rssi} dBm",
        longest_gap.as_secs_f64()
    );
    if missing.is_empty() {
        println!("RNODE BULK TX PROBE PASSED: no loss, no stall");
    } else {
        let head: Vec<u32> = missing.iter().copied().take(20).collect();
        println!("missing {} frames; first missing: {head:?}", missing.len());
        println!("RNODE BULK TX PROBE FAILED");
        listen.abort();
        std::process::exit(1);
    }
    listen.abort();
    sender.shutdown().await?;
    Ok(())
}
