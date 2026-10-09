//! The RNode host protocol, device side: the board as a radio stock Reticulum drives.
//!
//! `tulle::rnode` is the other half of this conversation, and both halves come from the
//! same place: black-box captures of RNS 1.3.8 driving RNode firmware 1.86 through a serial
//! tee (`crates/tulle/tests/fixtures/rnode_serial_capture.json`). The GPL firmware source is
//! never read; what a device must answer is what a device was observed to answer.
//!
//! The captured conversation, in full:
//!
//! - The host opens with `DETECT(0x73)`, then `FW_VERSION`, `PLATFORM`, `MCU`. The device
//!   answers `DETECT(0x46)`, its version as two bytes, and one byte each for platform and MCU.
//! - Then `FREQUENCY`, `BANDWIDTH`, `TXPOWER`, `SF`, `CR`, each echoed back verbatim, and
//!   `RADIO_STATE(1)`, echoed once the radio is actually on.
//! - Transmit is `DATA` framing the packet. Receive is a triplet: `STAT_RSSI`, `STAT_SNR`,
//!   then `DATA` with the packet verbatim.
//!
//! # Crossing to stock RNode hardware: settled 2026-08-07
//!
//! This used to say the two firmwares had been swept across seven sync words and inverted IQ
//! in both directions without ever crossing
//! (`design_docs/2026-07-25_rnode_direct_phy_rf_opacity.md`), and that reaching stock RNode
//! hardware was an open question. **They cross.** A stock RNode 1.86 on a T114, driven over
//! BLE by an iPhone, was received by a V4 on this channel at the trunk profile: RSSI -6 dBm,
//! valid Reticulum announces, repeatedly.
//!
//! The July sweep predates the SX126x CRC fix, when the driver handed CRC-failed packets up
//! as good frames. A probe asking "did my smoke frame arrive intact?" answers no in that
//! world whether or not the radios are hearing each other, so the sweep could not have
//! distinguished opacity from corruption. Retest anything else that sweep concluded.
//!
//! One artefact remains and is **not** ours: frames from that peer carry one spurious byte
//! before the packet. Because the driver now rejects CRC failures and these frames pass, the
//! bytes are exactly what was transmitted, so the extra byte is in the sender rather than in
//! our demodulation. See `design_docs/2026-08-07_rnode_rx_leading_byte.md`.
//!
//! What it does buy is the thing that was missing: Sideband, MeshChat and NomadNet drive an
//! RNode, so they drive this board, with no host-side shim in between.

use selvage::kiss;

mod pending;

pub use pending::{PREAMBLE_SYMBOLS, Pending, SYNC_WORD};

/// Command bytes, named per the public constant table and observed on the wire.
pub mod cmd {
    pub const DATA: u8 = 0x00;
    pub const FREQUENCY: u8 = 0x01;
    pub const BANDWIDTH: u8 = 0x02;
    pub const TXPOWER: u8 = 0x03;
    pub const SF: u8 = 0x04;
    pub const CR: u8 = 0x05;
    pub const RADIO_STATE: u8 = 0x06;
    pub const DETECT: u8 = 0x08;
    pub const LEAVE: u8 = 0x0A;
    pub const ST_ALOCK: u8 = 0x0B;
    pub const LT_ALOCK: u8 = 0x0C;
    pub const READY: u8 = 0x0F;
    pub const STAT_RSSI: u8 = 0x23;
    pub const STAT_SNR: u8 = 0x24;
    pub const PLATFORM: u8 = 0x48;
    pub const MCU: u8 = 0x49;
    pub const FW_VERSION: u8 = 0x50;
    pub const RESET: u8 = 0x55;
    pub const ERROR: u8 = 0x90;
}

/// Detect request and response magic bytes.
pub const DETECT_REQ: u8 = 0x73;
pub const DETECT_RESP: u8 = 0x46;

/// RSSI on the wire is offset: `dBm = raw - 157`.
pub const RSSI_OFFSET: i16 = 157;

/// The protocol version this device answers as, `1.86`, from the capture.
///
/// A statement about the wire, not about the firmware: it says "this device speaks the
/// protocol as captured from 1.86", which is the only claim these bytes can carry. The
/// board's own version is on its banner, where a person reads it.
pub const FW_VERSION: [u8; 2] = [0x01, 0x56];

/// Platform and MCU bytes, as captured. Both boards in the fixtures answered these two
/// values, so they are reported rather than derived.
pub const PLATFORM: u8 = 0x70;
pub const MCU: u8 = 0x71;

/// The largest frame the host protocol carries: RNS's RNode HW_MTU, a 500-byte packet plus
/// an 8-byte IFAC (`RNodeInterface.py` 110, 195).
///
/// Larger than this radio's 255-byte air frame on purpose: a host that sends 508 bytes must
/// be *told* so, and a deframer bounded at 255 would silently resync instead. See
/// [`MAX_AIR_FRAME`].
pub const MAX_FRAME: usize = 508;

/// The largest frame this radio can actually put on the air.
///
/// The 255/500 fork the plan names. Carrying longer packets needs the fragmentation lane;
/// until it exists, an over-long transmit is dropped and counted rather than truncated.
pub const MAX_AIR_FRAME: usize = selvage::MAX_RADIO_FRAME_LEN;

/// Bytes of buffer a deframer needs: the largest frame plus its command byte.
pub const DEFRAME_BUF: usize = MAX_FRAME + 1;

/// A KISS deframer sized for this protocol.
pub type Deframer = kiss::Deframer<DEFRAME_BUF>;

/// What the host asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command<'a> {
    /// `DETECT`, carrying the request magic. A device answers only the magic it knows.
    Detect(u8),
    FirmwareVersion,
    Platform,
    Mcu,
    Frequency(u32),
    Bandwidth(u32),
    TxPower(u8),
    SpreadingFactor(u8),
    /// The denominator: 5 through 8 for 4/5 through 4/8.
    CodingRate(u8),
    /// Turn the radio on or off. This is what commits the settings above.
    RadioState(bool),
    /// A short- (`long == false`) or long-term airtime limit, in hundredths of a percent.
    AirtimeLock {
        long: bool,
        centi: u16,
    },
    /// The host is detaching (`RNodeInterface.py` 496-500).
    Leave,
    /// Flow control's READY. A device sends it; a host has no reason to.
    Ready,
    /// A packet to put on the air.
    Data(&'a [u8]),
    /// A command this device does not implement, or one whose payload did not decode.
    ///
    /// Named rather than dropped: a host probing something unimplemented should show up on a
    /// counter, not look like a dead port.
    Unhandled(u8),
}

/// Read one deframed KISS frame as a command.
pub fn decode(frame: &[u8]) -> Option<Command<'_>> {
    let (&command, payload) = frame.split_first()?;
    let byte = || payload.first().copied();
    let half = || {
        let bytes: [u8; 2] = payload.get(..2)?.try_into().ok()?;
        Some(u16::from_be_bytes(bytes))
    };
    let word = || {
        let bytes: [u8; 4] = payload.get(..4)?.try_into().ok()?;
        Some(u32::from_be_bytes(bytes))
    };
    Some(match command {
        cmd::DETECT => Command::Detect(byte().unwrap_or(0)),
        cmd::FW_VERSION => Command::FirmwareVersion,
        cmd::PLATFORM => Command::Platform,
        cmd::MCU => Command::Mcu,
        cmd::FREQUENCY => match word() {
            Some(hz) => Command::Frequency(hz),
            None => Command::Unhandled(command),
        },
        cmd::BANDWIDTH => match word() {
            Some(hz) => Command::Bandwidth(hz),
            None => Command::Unhandled(command),
        },
        cmd::TXPOWER => match byte() {
            Some(dbm) => Command::TxPower(dbm),
            None => Command::Unhandled(command),
        },
        cmd::SF => match byte() {
            Some(sf) => Command::SpreadingFactor(sf),
            None => Command::Unhandled(command),
        },
        cmd::CR => match byte() {
            Some(cr) => Command::CodingRate(cr),
            None => Command::Unhandled(command),
        },
        cmd::RADIO_STATE => Command::RadioState(byte() == Some(1)),
        cmd::ST_ALOCK | cmd::LT_ALOCK => match half() {
            Some(centi) => Command::AirtimeLock {
                long: command == cmd::LT_ALOCK,
                centi,
            },
            None => Command::Unhandled(command),
        },
        cmd::LEAVE => Command::Leave,
        cmd::READY => Command::Ready,
        cmd::DATA => Command::Data(payload),
        other => Command::Unhandled(other),
    })
}

/// A fixed device-to-host reply payload. Four bytes covers every one this device gives; the
/// two that are longer are a received packet and nothing else.
pub type Payload = heapless::Vec<u8, 4>;

/// The device's answer to a command that needs no radio.
///
/// The probes, and the echo of each setting. Airtime limits are echoed as 0 (none), since
/// this device leaves duty to the region profile and does not enforce them; RNS only
/// records the echo (`RNodeInterface.py` 896-925). `None` for the two commands that touch
/// hardware, which the channel owns: `RADIO_STATE` commits a profile and `DATA` puts a frame
/// on the air. `LEAVE` and `READY` take no answer: the host that sent LEAVE has gone, and
/// READY is the device's own word.
///
/// Settings are echoed from the *decoded* value rather than by copying the bytes back, so a
/// decode that misread a field would show up as a wrong echo. The capture is what says which
/// of those is right, and the gold test compares against it.
pub fn answer(command: &Command<'_>) -> Option<(u8, Payload)> {
    let one = |byte: u8| Payload::from_slice(&[byte]).unwrap_or_default();
    let four = |word: u32| Payload::from_slice(&word.to_be_bytes()).unwrap_or_default();
    Some(match *command {
        Command::Detect(magic) if magic == DETECT_REQ => (cmd::DETECT, one(DETECT_RESP)),
        Command::Detect(_) => return None,
        Command::FirmwareVersion => (
            cmd::FW_VERSION,
            Payload::from_slice(&FW_VERSION).unwrap_or_default(),
        ),
        Command::Platform => (cmd::PLATFORM, one(PLATFORM)),
        Command::Mcu => (cmd::MCU, one(MCU)),
        Command::Frequency(hz) => (cmd::FREQUENCY, four(hz)),
        Command::Bandwidth(hz) => (cmd::BANDWIDTH, four(hz)),
        Command::TxPower(dbm) => (cmd::TXPOWER, one(dbm)),
        Command::SpreadingFactor(sf) => (cmd::SF, one(sf)),
        Command::CodingRate(cr) => (cmd::CR, one(cr)),
        // This board enforces no airtime lock, so it reports none. RNS records the echo and
        // never validates it (`RNodeInterface.py` 667-692, 896-925).
        Command::AirtimeLock { long, .. } => (
            if long { cmd::LT_ALOCK } else { cmd::ST_ALOCK },
            Payload::from_slice(&[0, 0]).unwrap_or_default(),
        ),
        Command::RadioState(_)
        | Command::Data(_)
        | Command::Leave
        | Command::Ready
        | Command::Unhandled(_) => return None,
    })
}

/// RNS's `ERROR` codes this device reports (`RNodeInterface.py` 89-95).
pub mod error {
    /// A transmit failed in the radio: RNS restarts the interface.
    pub const TX_FAILED: u8 = 0x02;
    /// The radio did not finish in time: RNS records it and carries on.
    pub const MODEM_TIMEOUT: u8 = 0x06;
}

/// The `ERROR` code a refused transmit reports, if any.
///
/// RNS restarts the interface on every code but memory-low and modem-timeout
/// (`RNodeInterface.py` 1076-1090), so only a radio failure is reported. Per-frame refusals
/// (no region, spent duty, a busy channel) are dropped and counted: a lost frame is cheaper
/// than a lost link.
pub fn tx_error(code: u8) -> Option<u8> {
    match code {
        selvage::TX_RADIO_FAULT => Some(error::TX_FAILED),
        selvage::TX_TIMEOUT => Some(error::MODEM_TIMEOUT),
        _ => None,
    }
}

/// Encode one device-to-host frame: a command byte and its payload, KISS-framed.
pub fn encode(command: u8, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    kiss::encode_pair_into(&[command], payload, out)
}

/// Bytes an [`encode`] of `payload` can need, worst case: every byte escaped.
pub const fn encoded_max(payload_len: usize) -> usize {
    2 * (payload_len + 1) + 2
}

/// The wire value for a received frame's RSSI.
pub fn rssi_wire(dbm: i16) -> u8 {
    (dbm + RSSI_OFFSET).clamp(0, 255) as u8
}

/// The wire value for a received frame's SNR: quarter-dB, signed.
pub fn snr_wire(db: i16) -> u8 {
    db.saturating_mul(4).clamp(-128, 127) as i8 as u8
}

#[cfg(test)]
mod tests;
