//! Two-witness probe: one RNode floods, two independent RNodes listen.
//!
//! This separates the two explanations for the bulk-frame loss recorded in
//! `design_docs/2026-07-26_rnode_bulk_frame_loss.md`. If both witnesses miss
//! the same sequence numbers, the sender never put those frames on the air.
//! If each witness misses a different set, the frames were transmitted and
//! the loss is on the receive side or in the channel.
//!
//! Usage:
//! `cargo run --features serial-async --example rnode_witness_probe -- COM5 COM6 COM7 [count] [frame_len] [bw_khz] [pace_ms]`

use std::time::Duration;

use tulle::airtime::AirtimeBudget;
use tulle::lora::{CodingRate, LoRaParams};
use tulle::serial::{RNodeSerialLink, SerialPumpConfig};

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

fn listen(
    mut radio: RNodeSerialLink,
    count: u32,
    label: &'static str,
) -> tokio::task::JoinHandle<(&'static str, Vec<bool>)> {
    tokio::spawn(async move {
        let mut seen = vec![false; count as usize];
        let mut heard = 0u32;
        // Ends on the first silence longer than STALL_TIMEOUT, or once every
        // sequence number has been accounted for.
        while let Ok(Some(frame)) = tokio::time::timeout(STALL_TIMEOUT, radio.recv()).await {
            if frame.frame.len() >= 4 {
                let seq = u32::from_be_bytes(frame.frame[..4].try_into().unwrap());
                if (seq as usize) < seen.len() && !seen[seq as usize] {
                    seen[seq as usize] = true;
                    heard += 1;
                }
            }
            if heard == count {
                break;
            }
        }
        (label, seen)
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let sender_port = args.next().unwrap_or_else(|| "COM5".into());
    let witness_a_port = args.next().unwrap_or_else(|| "COM6".into());
    let witness_b_port = args.next().unwrap_or_else(|| "COM7".into());
    let count: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(60);
    let frame_len: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(243);
    let bandwidth_hz: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(125) * 1_000;
    let pace_ms: u64 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(180);

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
    let budget = || AirtimeBudget::new(60_000, 60_000);

    let mut sender =
        RNodeSerialLink::open(&sender_port, params, budget(), SerialPumpConfig::default())?;
    let mut witness_a = RNodeSerialLink::open(
        &witness_a_port,
        params,
        budget(),
        SerialPumpConfig::default(),
    )?;
    let mut witness_b = RNodeSerialLink::open(
        &witness_b_port,
        params,
        budget(),
        SerialPumpConfig::default(),
    )?;
    tokio::time::timeout(Duration::from_secs(25), sender.wait_online()).await??;
    tokio::time::timeout(Duration::from_secs(25), witness_a.wait_online()).await??;
    tokio::time::timeout(Duration::from_secs(25), witness_b.wait_online()).await??;
    println!(
        "online: {sender_port} sends, {witness_a_port} and {witness_b_port} witness; \
         {count} frames of {frame_len} B at BW {} kHz",
        bandwidth_hz / 1000
    );

    let a = listen(witness_a, count, "A");
    let b = listen(witness_b, count, "B");

    let mut frame = vec![0u8; frame_len.max(4)];
    for seq in 0..count {
        frame[..4].copy_from_slice(&seq.to_be_bytes());
        fill(&mut frame[4..], seq.wrapping_mul(0x9e37_79b9));
        let airtime = sender.send(frame.clone()).await?;
        tokio::time::sleep(airtime + Duration::from_millis(pace_ms)).await;
    }
    println!("flood sent");

    let (_, seen_a) = a.await?;
    let (_, seen_b) = b.await?;

    let both = (0..count as usize)
        .filter(|&i| seen_a[i] && seen_b[i])
        .count();
    let only_a = (0..count as usize)
        .filter(|&i| seen_a[i] && !seen_b[i])
        .count();
    let only_b = (0..count as usize)
        .filter(|&i| !seen_a[i] && seen_b[i])
        .count();
    let neither = (0..count as usize)
        .filter(|&i| !seen_a[i] && !seen_b[i])
        .count();

    println!(
        "heard by both: {both}; only {witness_a_port}: {only_a}; only {witness_b_port}: {only_b}; neither: {neither}"
    );
    let disagreements = only_a + only_b;
    if neither > 0 && disagreements == 0 {
        println!(
            "VERDICT: the witnesses agree exactly. The missing frames were never transmitted."
        );
    } else if disagreements > 0 && neither == 0 {
        println!(
            "VERDICT: every transmitted frame reached at least one witness. The loss is on the receive side."
        );
    } else {
        println!(
            "VERDICT: mixed. {neither} frames reached neither witness (sender-side or channel), \
             {disagreements} reached exactly one (receive-side)."
        );
    }
    Ok(())
}
