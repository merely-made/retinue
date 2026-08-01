//! Heltec Mesh Node T114 Rev. 2.x board facts.
//!
//! Heltec's Rev. 2.1 schematic and pin map assign:
//! - TFT power/reset/DC/MOSI/SCK/CS/backlight: P0.03/P0.02/P0.12/
//!   P1.09/P1.08/P0.11/P0.15
//! - green status LED: P1.03, active low
//! - SX1262 NSS/SCK/MOSI/MISO/reset/busy/DIO1:
//!   P0.24/P0.19/P0.22/P0.23/P0.25/P0.17/P0.20
//!
//! Published sources disagree about the user switch: Heltec's Rev. 2.1
//! schematic names P1.11 while the maintained board variant names P1.10.
//! The adapter listens to both otherwise-unused pins until the fitted revision
//! is confirmed.

use radio_face::{HostState, LocalStatus, PowerSource, RadioProfile, RadioState, SleepState, Text};
use selvage::MESHTASTIC_SYNC_WORD;

pub const DEFAULT_FREQUENCY_HZ: u32 = 906_875_000;
pub const DEFAULT_BANDWIDTH_HZ: u32 = 250_000;
pub const DEFAULT_SPREADING_FACTOR: u8 = 11;
pub const DEFAULT_CODING_RATE_DENOMINATOR: u8 = 5;
pub const DEFAULT_TX_POWER_DBM: i8 = 17;

pub fn initial_status() -> LocalStatus {
    LocalStatus {
        board: Text::from_truncated("HELTEC T114"),
        firmware: Text::from_truncated(concat!("PHY ", env!("CARGO_PKG_VERSION"))),
        radio: RadioState::Booting,
        host: HostState::Detached,
        power_source: PowerSource::Usb,
        display_on: true,
        sleep: SleepState::Disabled,
        profile: RadioProfile {
            frequency_hz: Some(DEFAULT_FREQUENCY_HZ),
            bandwidth_hz: Some(DEFAULT_BANDWIDTH_HZ),
            spreading_factor: Some(DEFAULT_SPREADING_FACTOR),
            coding_rate_denominator: Some(DEFAULT_CODING_RATE_DENOMINATOR),
            tx_power_dbm: Some(DEFAULT_TX_POWER_DBM),
            sync_word: Some(MESHTASTIC_SYNC_WORD),
            name: Text::from_truncated("LONGFAST"),
        },
        ..Default::default()
    }
}
