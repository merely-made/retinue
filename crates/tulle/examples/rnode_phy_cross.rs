//! Do the RNode channel and direct PHY share the air?
//!
//! `rnode_bulk_probe` says a direct-PHY listener cannot serve as its receiver, because stock
//! RNode firmware 1.86 and this project's direct-PHY firmware were swept against each other
//! and never crossed (`design_docs/2026-07-25_rnode_direct_phy_rf_opacity.md`). That finding
//! is about *stock* RNode. Our own RNode channel is a different device: it speaks the same
//! host protocol but programs the radio itself, with the sync word and preamble every other
//! personality here uses. So the two should cross, and this is what asks.
//!
//! It is also the instrument that would catch the opposite result. If our RNode channel ever
//! stops being heard by our own direct-PHY boards, the personalities have drifted apart on
//! the air while still passing every desk test, and nothing else in the harness would notice.
//!
//! Counted in both directions, per the receipt rule: a single pass on a shared ISM band is
//! not a receipt.
//!
//! Usage:
//! `cargo run --features serial-async --example rnode_phy_cross -- COM10 COM6 [rounds] [len] [freq_hz] [bw_hz]`

use std::time::Duration;

use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use tulle::lora::{CodingRate, LoRaParams};
use tulle::serial::{RNodeSerialLink, SerialPumpConfig};

/// The on-air settings the RNode host protocol cannot reach, which the board therefore
/// chooses for itself. They have to be spelled out on the direct-PHY side to match.
const SYNC_WORD: u8 = 0x12;
const PREAMBLE_SYMBOLS: u16 = 16;

fn tagged(round: u32, len: usize, label: u8) -> Vec<u8> {
    let mut frame = vec![label; len];
    frame[..4].copy_from_slice(&round.to_be_bytes());
    frame
}

/// Why a round did not produce its frame.
///
/// Three outcomes, not two, because they have different causes and the counts alone cannot
/// tell them apart. A first run of this instrument scored one direction 5 of 8 while the
/// board's own `rxok` counter said it had received all 8 — so the loss was above the board,
/// and naming *where* is the difference between a radio problem and a cable problem.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The frame arrived byte-exact.
    Heard,
    /// Nothing arrived before the deadline.
    Silent,
    /// The pump's receive channel closed: the host side gave up, not the radio.
    PumpGone,
}

/// Where a frame first differs from the one expected, as text for a log line.
///
/// Empty when the lengths differ, because then the interesting fact is the length.
fn difference(got: &[u8], wanted: &[u8]) -> String {
    if got.len() != wanted.len() {
        return format!(", length {} not {}", got.len(), wanted.len());
    }
    match got.iter().zip(wanted).position(|(a, b)| a != b) {
        Some(at) => {
            let differing: Vec<String> = got
                .iter()
                .zip(wanted)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(index, (a, b))| format!("{index}:{a:02x}/{b:02x}"))
                .collect();
            format!(
                ", DAMAGED first at byte {at} ({} of {} bytes differ) [{}]",
                differing.len(),
                got.len(),
                differing.join(" "),
            )
        }
        None => String::new(),
    }
}

/// One end of the link, whichever protocol it speaks.
///
/// The two pumps already have the same `recv`, so this exists only to let one waiting loop
/// serve both rather than being written twice with a chance to diverge.
#[allow(async_fn_in_trait)]
trait Listen {
    async fn next_frame(&mut self) -> Option<Vec<u8>>;
}

impl Listen for RNodeSerialLink {
    async fn next_frame(&mut self) -> Option<Vec<u8>> {
        self.recv().await.map(|received| received.frame)
    }
}

impl Listen for DirectPhySerialLink {
    async fn next_frame(&mut self) -> Option<Vec<u8>> {
        self.recv().await.map(|received| received.frame)
    }
}

/// Wait for one specific frame, ignoring whatever else the shared band carries.
async fn await_frame<L: Listen>(link: &mut L, wanted: &[u8], direction: &str) -> Outcome {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            match link.next_frame().await {
                Some(frame) if frame == wanted => return Outcome::Heard,
                // Named, not just counted. Every frame this bench puts on the air carries its
                // round in the first four bytes and its direction in the fill, so an
                // unexpected arrival says which one it is instead of being written off as
                // interference — and if it is the frame being waited for, arriving damaged,
                // this prints where it differs. A frame that is the right length and the
                // right round but the wrong bytes is a different bug from one that is late.
                Some(frame) => println!(
                    "  ({direction}: ignoring a {}-byte frame, round {:?} fill 0x{:02x}{})",
                    frame.len(),
                    frame
                        .get(..4)
                        .map(|tag| u32::from_be_bytes(tag.try_into().unwrap())),
                    frame.get(8).copied().unwrap_or(0),
                    difference(&frame, wanted),
                ),
                None => return Outcome::PumpGone,
            }
        }
    })
    .await
    .unwrap_or(Outcome::Silent)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let rnode_port = args.next().unwrap_or_else(|| "COM10".into());
    let phy_port = args.next().unwrap_or_else(|| "COM6".into());
    let rounds: u32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(8);
    let len: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(64);
    let frequency_hz: u32 = args
        .next()
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(915_000_000);
    let bandwidth_hz: u32 = args
        .next()
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(125_000);

    let rnode_params = LoRaParams {
        spreading_factor: 8,
        bandwidth_hz,
        coding_rate: CodingRate::Cr45,
        frequency_hz,
        tx_power_dbm: 7,
        // Airtime accounting only: the host protocol has no preamble command, so this must
        // say what the board actually uses or the pacing is wrong.
        preamble_syms: PREAMBLE_SYMBOLS,
        explicit_header: true,
        crc: true,
    };
    let phy_profile = PhyProfile {
        frequency_hz,
        bandwidth_hz,
        spreading_factor: 8,
        coding_rate_denominator: 5,
        preamble_symbols: PREAMBLE_SYMBOLS,
        sync_word: SYNC_WORD,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 7,
    };

    println!("opening {rnode_port} as an RNode and {phy_port} as direct PHY");
    let mut rnode = RNodeSerialLink::open(
        &rnode_port,
        rnode_params,
        AirtimeBudget::new(60_000, 60_000),
        SerialPumpConfig::default(),
    )?;
    let mut phy = DirectPhySerialLink::open(
        &phy_port,
        phy_profile,
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    let firmware = tokio::time::timeout(Duration::from_secs(25), rnode.wait_online()).await??;
    tokio::time::timeout(Duration::from_secs(20), phy.wait_online()).await??;
    println!("online: rnode firmware={firmware:?}, direct phy ready");
    println!(
        "profile: {} Hz, BW {} kHz, SF8, CR4/5, sync 0x{SYNC_WORD:02x}, preamble {PREAMBLE_SYMBOLS}, \
         {len}-byte frames, {rounds} rounds each way",
        frequency_hz,
        bandwidth_hz / 1000,
    );

    let (mut to_phy, mut to_rnode) = (0_u32, 0_u32);
    for round in 0..rounds {
        let outbound = tagged(round, len, 0xA5);
        rnode.send(outbound.clone()).await?;
        let forward = await_frame(&mut phy, &outbound, "rnode->phy").await;
        if forward == Outcome::Heard {
            to_phy += 1;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;

        let inbound = tagged(round, len, 0x5A);
        phy.send(inbound.clone()).await?;
        let reverse = await_frame(&mut rnode, &inbound, "phy->rnode").await;
        if reverse == Outcome::Heard {
            to_rnode += 1;
        }
        println!(
            "round {round}: rnode->phy {to_phy}/{} ({forward:?}), phy->rnode {to_rnode}/{} ({reverse:?})",
            round + 1,
            round + 1,
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    println!("\nrnode -> phy: {to_phy} of {rounds}");
    println!("phy -> rnode: {to_rnode} of {rounds}");
    rnode.shutdown().await?;
    phy.shutdown().await?;
    if to_phy == 0 || to_rnode == 0 {
        return Err("a direction never crossed".into());
    }
    Ok(())
}
