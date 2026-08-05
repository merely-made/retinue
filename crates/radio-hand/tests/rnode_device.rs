//! Gold test: the board's RNode device half against the live-hardware capture.
//!
//! `tulle`'s `rnode_capture` test asserts that our *host* asks what RNS asked. This is the
//! mirror: that our *device* answers what an RNode answered. The fixture holds both sides of
//! one real conversation — RNS 1.3.8 driving RNode firmware 1.86 through a serial tee — so
//! the same bytes settle both halves, and neither is checked against the other's assumptions.
//!
//! The fixture is read from `tulle`'s copy rather than a third one. It is the oracle, not
//! either crate's property, and a capture duplicated once more is a capture that can drift.

use radio_hand::rnode::{self, Command, cmd};
use selvage::kiss;
use serde_json::Value;

/// The device-to-host frames a real RNode sends unprompted, which this device does not.
///
/// Channel utilisation, battery, and PHY parameters: `tulle` ignores all three while driving
/// real hardware, so a host that only needs the link does not depend on them. Skipped here
/// rather than silently tolerated, so the omission stays visible.
const UNSOLICITED: [u8; 3] = [0x25, 0x26, 0x27];

fn fixture() -> Value {
    let path = format!(
        "{}/../tulle/tests/fixtures/rnode_serial_capture.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Every complete frame in one direction, in event order.
fn frames(cap: &Value, direction: &str) -> Vec<Vec<u8>> {
    let bytes: Vec<u8> = cap["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["dir"] == direction)
        .flat_map(|e| hex(e["hex"].as_str().unwrap()))
        .collect();

    let mut deframer = kiss::Deframer::<1024>::new();
    let mut out = Vec::new();
    for byte in bytes {
        if deframer.push(byte) {
            out.push(deframer.frame().to_vec());
        }
    }
    out
}

/// What this device would put on the wire in answer, encoded exactly as the channel encodes
/// it.
fn our_answer(frame: &[u8]) -> Option<Vec<u8>> {
    let command = rnode::decode(frame)?;
    let (marker, payload) = rnode::answer(&command)?;
    let mut out = [0_u8; 64];
    let len = rnode::encode(marker, &payload, &mut out)?;
    Some(out[..len].to_vec())
}

/// The contents of a single encoded frame: what the far end will read out of it.
///
/// Comparing wire bytes directly would compare *escaping* rather than meaning, and this
/// conversation escapes for real — 915 MHz is `36 89 CA C0`, whose last byte is the frame
/// delimiter.
fn contents(wire: &[u8]) -> Vec<u8> {
    let mut deframer = kiss::Deframer::<1024>::new();
    let mut out = Vec::new();
    for &byte in wire {
        if deframer.push(byte) {
            out = deframer.frame().to_vec();
        }
    }
    out
}

/// Every command RNS sent during init is one this device implements.
///
/// The failure this guards is the quiet one: a host probe answered with silence looks exactly
/// like a dead port, and the only way to tell them apart is to have checked.
#[test]
fn every_command_the_oracle_sent_is_one_this_device_knows() {
    let cap = fixture();
    for frame in frames(&cap, "host->rnode") {
        let command = rnode::decode(&frame).expect("a captured frame is never empty");
        assert!(
            !matches!(command, Command::Unhandled(_)),
            "unhandled command 0x{:02x} in the capture",
            frame[0],
        );
    }
}

/// Our answers match the real device's, frame for frame and byte for byte.
///
/// Both directions are walked in capture order, skipping the unsolicited frames a real device
/// interleaves. What remains is the conversation proper: detect, three probes, five settings
/// echoes.
#[test]
fn our_answers_match_the_oracle_device_frame_for_frame() {
    let cap = fixture();
    let asked = frames(&cap, "host->rnode");
    let answered: Vec<Vec<u8>> = frames(&cap, "rnode->host")
        .into_iter()
        .filter(|frame| !UNSOLICITED.contains(&frame[0]))
        .collect();

    let mut theirs = answered.iter();
    let mut compared = 0;
    for frame in &asked {
        // The two commands that need a radio are the channel's, not this half's.
        if frame[0] == cmd::RADIO_STATE || frame[0] == cmd::DATA {
            continue;
        }
        let ours = our_answer(frame).expect("every probe and setting is answered");
        let theirs = theirs.next().expect("the capture answered every one");
        assert_eq!(
            contents(&ours),
            *theirs,
            "answer to 0x{:02x} differs from the oracle",
            frame[0],
        );
        compared += 1;
    }
    assert!(
        compared >= 9,
        "detect, three probes, five settings: {compared}"
    );
}

/// The frequency echo escapes on the wire, because 915 MHz ends in the frame delimiter.
///
/// Worth pinning on its own: the escape path is not an edge case here, it is the very first
/// setting every US host sends, and a device that got it wrong would fail at hello.
#[test]
fn the_frequency_echo_escapes_the_delimiter_inside_it() {
    let command = rnode::decode(&[cmd::FREQUENCY, 0x36, 0x89, 0xca, 0xc0]).unwrap();
    let (marker, payload) = rnode::answer(&command).unwrap();
    let mut out = [0_u8; 32];
    let len = rnode::encode(marker, &payload, &mut out).unwrap();

    assert_eq!(
        &out[..len],
        &[
            kiss::FEND,
            cmd::FREQUENCY,
            0x36,
            0x89,
            0xca,
            kiss::FESC,
            kiss::TFEND,
            kiss::FEND
        ],
    );
    assert_eq!(
        contents(&out[..len]),
        vec![cmd::FREQUENCY, 0x36, 0x89, 0xca, 0xc0]
    );
}

/// The settings RNS committed rebuild into the profile it asked for.
///
/// The capture's own config block is the assertion: 915 MHz, 125 kHz, SF8, CR 4/5, 7 dBm.
#[test]
fn the_captured_settings_rebuild_the_profile_the_host_asked_for() {
    let cap = fixture();
    let mut pending = rnode::Pending::new();
    for frame in frames(&cap, "host->rnode") {
        if let Some(command) = rnode::decode(&frame) {
            pending.accept(&command);
        }
    }

    let profile = pending.profile().expect("the capture sets every field");
    let wanted = &cap["config"];
    assert_eq!(
        profile.frequency_hz as u64,
        wanted["frequency"].as_u64().unwrap()
    );
    assert_eq!(
        profile.bandwidth_hz as u64,
        wanted["bandwidth"].as_u64().unwrap()
    );
    assert_eq!(
        u64::from(profile.spreading_factor),
        wanted["spreadingfactor"].as_u64().unwrap()
    );
    assert_eq!(
        u64::from(profile.coding_rate_denominator),
        wanted["codingrate"].as_u64().unwrap()
    );
    assert_eq!(
        profile.tx_power_dbm as i64,
        wanted["txpower"].as_i64().unwrap()
    );

    // The settings the host protocol cannot reach are ours, and this is where that shows.
    assert_eq!(profile.sync_word, rnode::SYNC_WORD);
    assert_eq!(profile.preamble_symbols, rnode::PREAMBLE_SYMBOLS);
    assert!(profile.explicit_header && profile.crc && !profile.invert_iq);
}

/// A real transmitted packet from the capture is one this radio can carry.
///
/// The 255/500 fork made concrete: the announce RNS actually sent is 167 bytes, so the frames
/// that matter for finding peers fit. What does not fit is refused, never truncated.
#[test]
fn the_captured_transmit_fits_the_air_frame() {
    let cap = fixture();
    let sent: Vec<Vec<u8>> = frames(&cap, "host->rnode")
        .into_iter()
        .filter(|frame| frame[0] == cmd::DATA)
        .collect();

    assert!(!sent.is_empty(), "the capture transmits at least once");
    for frame in sent {
        let packet = &frame[1..];
        assert!(
            packet.len() <= rnode::MAX_AIR_FRAME,
            "a {}-byte packet does not fit {}",
            packet.len(),
            rnode::MAX_AIR_FRAME,
        );
    }
}

/// The receive triplet this device sends is the one the RX capture carried: stats first, then
/// the packet, with the RSSI and SNR encodings the host reverses.
#[test]
fn the_receive_triplet_encodes_as_the_capture_did() {
    let mut out = [0_u8; 8];

    let len = rnode::encode(cmd::STAT_RSSI, &[rnode::rssi_wire(-60)], &mut out).unwrap();
    assert_eq!(&out[..len], &[kiss::FEND, cmd::STAT_RSSI, 0x61, kiss::FEND]);

    // Raw 0x3b is 14.75 dB; the radio reports whole dB, so 14 encodes as 56.
    let len = rnode::encode(cmd::STAT_SNR, &[rnode::snr_wire(14)], &mut out).unwrap();
    assert_eq!(&out[..len], &[kiss::FEND, cmd::STAT_SNR, 56, kiss::FEND]);
}
