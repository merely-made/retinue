//! The settings a host accumulates before `RADIO_STATE` commits them.

use selvage::PhyProfile;

use super::Command;

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
