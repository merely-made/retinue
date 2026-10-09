//! A `tulle` RNode host against this device's RNode half: what the board answers must keep
//! the host's link up through per-frame refusals and drop it only on a radio failure.

use radio_hand::rnode::{self, Command, Pending, cmd};
use selvage::kiss::Deframer;
use tulle::lora::{CodingRate, LoRaParams};
use tulle::modem::Modem;
use tulle::rnode::{Fault, RNode, RNodeConfig};

/// The board's RNode channel minus the radio: every DATA meets the executive result `tx`.
struct Board {
    deframer: Deframer<{ rnode::DEFRAME_BUF }>,
    pending: Pending,
    tx: u8,
}

impl Board {
    fn new() -> Self {
        Self {
            deframer: Deframer::new(),
            pending: Pending::new(),
            tx: selvage::TX_ACCEPTED,
        }
    }

    /// Feed host bytes; return the device's reply bytes.
    fn serve(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut replies = Vec::new();
        for &byte in bytes {
            if !self.deframer.push(byte) {
                continue;
            }
            let Some(command) = rnode::decode(self.deframer.frame()) else {
                continue;
            };
            self.pending.accept(&command);
            let reply = match command {
                Command::RadioState(on) => {
                    let up = on && self.pending.profile().is_some();
                    Some((cmd::RADIO_STATE, vec![u8::from(up)]))
                }
                Command::Data(_) => rnode::tx_error(self.tx).map(|code| (cmd::ERROR, vec![code])),
                other => rnode::answer(&other).map(|(marker, p)| (marker, p.to_vec())),
            };
            if let Some((marker, payload)) = reply {
                let mut out = vec![0; rnode::encoded_max(payload.len())];
                let len = rnode::encode(marker, &payload, &mut out).unwrap();
                replies.extend_from_slice(&out[..len]);
            }
        }
        replies
    }
}

fn host_online(board: &mut Board) -> RNode {
    let params = LoRaParams {
        spreading_factor: 7,
        bandwidth_hz: 125_000,
        coding_rate: CodingRate::Cr45,
        frequency_hz: 869_525_000,
        tx_power_dbm: 14,
        preamble_syms: 8,
        explicit_header: true,
        crc: true,
    };
    // Airtime limits the board echoes as 0: recorded, not compared, so the host comes up.
    let mut host = RNode::with_config(RNodeConfig {
        st_alock: Some(209),
        lt_alock: Some(10_000),
        ..RNodeConfig::new(params)
    });
    host.start();
    let replies = board.serve(&host.take_outbound());
    host.on_serial(&replies);
    assert_eq!(host.take_fault(), None);
    assert!(host.is_online());
    host
}

fn send(host: &mut RNode, board: &mut Board, tx: u8) {
    board.tx = tx;
    host.enqueue(b"packet").unwrap();
    let replies = board.serve(&host.take_outbound());
    host.on_serial(&replies);
}

#[test]
fn per_frame_refusals_keep_the_host_online() {
    let mut board = Board::new();
    let mut host = host_online(&mut board);
    for refusal in [
        selvage::TX_OVER_DUTY,
        selvage::TX_CHANNEL_BUSY,
        selvage::TX_NO_REGION,
    ] {
        send(&mut host, &mut board, refusal);
        assert_eq!(host.take_fault(), None, "refusal {refusal}");
        assert_eq!(host.take_last_error(), None);
        assert!(host.is_online());
    }
    send(&mut host, &mut board, selvage::TX_TIMEOUT);
    assert_eq!(host.take_fault(), None, "a timeout is recorded");
    assert_eq!(
        host.take_last_error(),
        Some(vec![rnode::error::MODEM_TIMEOUT])
    );
    assert!(host.is_online());
}

#[test]
fn a_radio_failure_takes_the_host_offline() {
    let mut board = Board::new();
    let mut host = host_online(&mut board);
    send(&mut host, &mut board, selvage::TX_RADIO_FAULT);
    assert_eq!(
        host.take_fault(),
        Some(Fault::Device(rnode::error::TX_FAILED))
    );
    assert!(!host.is_online());
}
