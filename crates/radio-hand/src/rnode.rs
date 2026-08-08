//! The RNode host protocol, device side: the board as a radio stock Reticulum drives.
//!
//! [`tulle::rnode`] is the other half of this conversation, and both halves come from the
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
//! # What this deliberately does not claim
//!
//! Speaking the host protocol is not the same as being on the air with a stock RNode. Those
//! two firmwares were swept against each other across seven sync words and inverted IQ in
//! both directions and never crossed
//! (`design_docs/2026-07-25_rnode_direct_phy_rf_opacity.md`); the host protocol exposes no
//! sync-word control, so whatever stock RNode programs stays invisible from here. This
//! channel therefore uses **our** on-air settings, which is what makes two boards on it hear
//! each other. Crossing to stock RNode hardware remains that open question, untouched.
//!
//! What it does buy is the thing that was missing: Sideband, MeshChat and NomadNet drive an
//! RNode, so they drive this board, with no host-side shim in between.

use selvage::PhyProfile;
use selvage::kiss;

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
    pub const STAT_RSSI: u8 = 0x23;
    pub const STAT_SNR: u8 = 0x24;
    pub const PLATFORM: u8 = 0x48;
    pub const MCU: u8 = 0x49;
    pub const FW_VERSION: u8 = 0x50;
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

/// The largest frame the host protocol carries.
///
/// Reticulum's packet MTU, and larger than this radio's 255-byte air frame. Kept at the
/// protocol's number on purpose: a host that sends 500 bytes must be *told* so, and a
/// deframer bounded at 255 would silently resync instead. See [`MAX_AIR_FRAME`].
pub const MAX_FRAME: usize = 500;

/// The largest frame this radio can actually put on the air.
///
/// The 255/500 fork the plan names. Carrying longer packets needs the fragmentation lane,
/// which is real work and not smuggled in here; until it exists, an over-long transmit is
/// refused with an `ERROR` frame rather than truncated.
pub const MAX_AIR_FRAME: usize = selvage::MAX_RADIO_FRAME_LEN;

/// The standard LoRa private-network sync word.
///
/// The same value every direct-PHY profile in this project uses, which is the point: the
/// host protocol has no sync-word command, so this is the device's own choice, and boards
/// that agree on it hear each other.
pub const SYNC_WORD: u8 = 0x12;

/// Preamble length, in symbols.
///
/// Eight, matching what stock RNode transmits, since this channel exists to be
/// interchangeable with one. The rest of this firmware's direct-PHY profiles use sixteen.
///
/// Changed from sixteen while chasing a one-byte frame shift, on the theory that a receiver
/// expecting a longer preamble locks late. **That theory was wrong** and is recorded here so
/// nobody re-derives it: with eight on both boards the shift was unchanged, and a
/// board-to-board control proved this receive path byte-exact, escapes included. The shift
/// is in what the peer transmits. Eight is kept because matching the thing we imitate is
/// right on its own, not because it fixed anything.
pub const PREAMBLE_SYMBOLS: u16 = 8;

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
        cmd::DATA => Command::Data(payload),
        other => Command::Unhandled(other),
    })
}

/// A fixed device-to-host reply payload. Four bytes covers every one this device gives; the
/// two that are longer are a received packet and nothing else.
pub type Payload = heapless::Vec<u8, 4>;

/// The device's answer to a command that needs no radio.
///
/// The probes, and the echo of each setting. `None` for the two commands that touch hardware,
/// which the channel owns: `RADIO_STATE` commits a profile and `DATA` puts a frame on the air.
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
        Command::RadioState(_) | Command::Data(_) | Command::Unhandled(_) => return None,
    })
}

/// Encode one device-to-host frame: a command byte and its payload, KISS-framed.
pub fn encode(command: u8, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    kiss::encode_pair_into(&[command], payload, out)
}

/// Bytes an [`encode`] of `payload` can need, worst case: every byte escaped.
pub const fn encoded_max(payload_len: usize) -> usize {
    2 * (payload_len + 1) + 2
}

/// The radio settings the host has asked for, accumulated until it says to apply them.
///
/// RNS sets five knobs as five separate commands and only then turns the radio on. Applying
/// each as it arrives would reconfigure the radio five times and spend four of those on a
/// channel nobody asked for; worse, the regulatory floor would reject a half-built profile
/// whose frequency had arrived but whose power had not. So they land here, and
/// `RADIO_STATE` is what commits them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pending {
    frequency_hz: Option<u32>,
    bandwidth_hz: Option<u32>,
    tx_power_dbm: Option<u8>,
    spreading_factor: Option<u8>,
    coding_rate: Option<u8>,
}

impl Pending {
    pub const fn new() -> Self {
        Self {
            frequency_hz: None,
            bandwidth_hz: None,
            tx_power_dbm: None,
            spreading_factor: None,
            coding_rate: None,
        }
    }

    /// Record a settings command. `true` if this command was one.
    pub fn accept(&mut self, command: &Command<'_>) -> bool {
        match *command {
            Command::Frequency(hz) => self.frequency_hz = Some(hz),
            Command::Bandwidth(hz) => self.bandwidth_hz = Some(hz),
            Command::TxPower(dbm) => self.tx_power_dbm = Some(dbm),
            Command::SpreadingFactor(sf) => self.spreading_factor = Some(sf),
            Command::CodingRate(cr) => self.coding_rate = Some(cr),
            _ => return false,
        }
        true
    }

    /// The profile to apply, once every field the host controls has arrived.
    ///
    /// `None` while anything is still missing, which is the honest answer: a radio brought up
    /// on defaults the host never chose is a radio on the wrong channel.
    pub fn profile(&self) -> Option<PhyProfile> {
        Some(PhyProfile {
            frequency_hz: self.frequency_hz?,
            bandwidth_hz: self.bandwidth_hz?,
            spreading_factor: self.spreading_factor?,
            coding_rate_denominator: self.coding_rate?,
            preamble_symbols: PREAMBLE_SYMBOLS,
            sync_word: SYNC_WORD,
            explicit_header: true,
            crc: true,
            invert_iq: false,
            // The host sends dBm as an unsigned byte; the executive clamps it to the region
            // and the hardware, so what arrives here is a request, never a setting.
            tx_power_dbm: i8::try_from(self.tx_power_dbm?).unwrap_or(i8::MAX),
        })
    }
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
mod tests {
    use super::*;

    #[test]
    fn the_captured_init_commands_decode() {
        assert_eq!(
            decode(&[cmd::DETECT, DETECT_REQ]),
            Some(Command::Detect(0x73))
        );
        assert_eq!(
            decode(&[cmd::FW_VERSION, 0x00]),
            Some(Command::FirmwareVersion)
        );
        assert_eq!(decode(&[cmd::PLATFORM, 0x00]), Some(Command::Platform));
        assert_eq!(decode(&[cmd::MCU, 0x00]), Some(Command::Mcu));
        // 0x3689cac0 = 915 MHz, 0x0001e848 = 125 kHz, both big-endian on the wire.
        assert_eq!(
            decode(&[cmd::FREQUENCY, 0x36, 0x89, 0xca, 0xc0]),
            Some(Command::Frequency(915_000_000))
        );
        assert_eq!(
            decode(&[cmd::BANDWIDTH, 0x00, 0x01, 0xe8, 0x48]),
            Some(Command::Bandwidth(125_000))
        );
        assert_eq!(decode(&[cmd::TXPOWER, 0x07]), Some(Command::TxPower(7)));
        assert_eq!(decode(&[cmd::SF, 0x08]), Some(Command::SpreadingFactor(8)));
        assert_eq!(decode(&[cmd::CR, 0x05]), Some(Command::CodingRate(5)));
        assert_eq!(
            decode(&[cmd::RADIO_STATE, 0x01]),
            Some(Command::RadioState(true))
        );
    }

    #[test]
    fn a_truncated_setting_is_unhandled_rather_than_guessed() {
        assert_eq!(
            decode(&[cmd::FREQUENCY, 0x36, 0x89]),
            Some(Command::Unhandled(cmd::FREQUENCY))
        );
        assert_eq!(decode(&[]), None);
    }

    /// An empty DATA frame is a command with an empty payload, not a missing one: the caller
    /// decides what to do with it, and refusing it here would hide it.
    #[test]
    fn data_carries_its_payload_verbatim() {
        assert_eq!(
            decode(&[cmd::DATA, 1, 2, 3]),
            Some(Command::Data(&[1, 2, 3]))
        );
        assert_eq!(decode(&[cmd::DATA]), Some(Command::Data(&[])));
    }

    #[test]
    fn a_profile_is_withheld_until_every_field_the_host_controls_has_arrived() {
        let mut pending = Pending::new();
        for command in [
            Command::Frequency(915_000_000),
            Command::Bandwidth(125_000),
            Command::TxPower(7),
            Command::SpreadingFactor(8),
        ] {
            assert!(pending.accept(&command));
            assert!(pending.profile().is_none(), "still incomplete");
        }
        assert!(pending.accept(&Command::CodingRate(5)));

        let profile = pending.profile().expect("complete");
        assert_eq!(profile.frequency_hz, 915_000_000);
        assert_eq!(profile.bandwidth_hz, 125_000);
        assert_eq!(profile.spreading_factor, 8);
        assert_eq!(profile.coding_rate_denominator, 5);
        assert_eq!(profile.tx_power_dbm, 7);
        assert_eq!(profile.sync_word, SYNC_WORD);
    }

    #[test]
    fn non_settings_commands_are_not_accepted_as_settings() {
        let mut pending = Pending::new();
        assert!(!pending.accept(&Command::Detect(DETECT_REQ)));
        assert!(!pending.accept(&Command::Data(&[1])));
        assert!(pending.profile().is_none());
    }

    /// The stat triplet's encodings, against the values the RX capture carried: raw 0x61 is
    /// -60 dBm and raw 0x3b is 14.75 dB.
    #[test]
    fn signal_reports_encode_as_the_capture_did() {
        assert_eq!(rssi_wire(-60), 0x61);
        assert_eq!(snr_wire(14), 56);
        assert_eq!(rssi_wire(-200), 0, "clamped rather than wrapped");
        assert_eq!(snr_wire(100), 127);
    }

    #[test]
    fn frames_encode_with_their_command_byte() {
        let mut out = [0_u8; 16];
        let len = encode(cmd::DETECT, &[DETECT_RESP], &mut out).unwrap();
        assert_eq!(
            &out[..len],
            &[kiss::FEND, cmd::DETECT, DETECT_RESP, kiss::FEND]
        );
    }

    #[test]
    fn the_worst_case_encoding_bound_holds() {
        let payload = [kiss::FEND; 8];
        let mut out = [0_u8; encoded_max(8)];
        let len = encode(kiss::FESC, &payload, &mut out).unwrap();
        assert_eq!(len, out.len(), "every byte escaped is the worst case");
    }
}
