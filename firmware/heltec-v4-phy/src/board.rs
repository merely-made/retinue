//! Heltec WiFi LoRa 32 V4.2 board facts.
//!
//! Heltec's V4.2 schematic and V4 pin map assign:
//! - OLED SDA/SCL/reset: GPIO17/GPIO18/GPIO21
//! - user button: GPIO0, active low with a pull-up
//! - white status LED: GPIO35, active high
//! - Vext control: GPIO36, active low
//! - SX1262 NSS/SCK/MOSI/MISO/reset/busy/DIO1: GPIO8..GPIO14

use radio_face::{HostState, LocalStatus, PowerSource, RadioProfile, RadioState, SleepState, Text};
use selvage::{MESHTASTIC_SYNC_WORD, PhyProfile};

pub const OLED_ADDRESS: u8 = 0x3c;
pub const DEFAULT_FREQUENCY_HZ: u32 = 906_875_000;
pub const DEFAULT_BANDWIDTH_HZ: u32 = 250_000;
pub const DEFAULT_SPREADING_FACTOR: u8 = 11;
pub const DEFAULT_CODING_RATE_DENOMINATOR: u8 = 5;
pub const DEFAULT_TX_POWER_DBM: i8 = 17;

pub fn initial_status() -> LocalStatus {
    LocalStatus {
        board: Text::from_truncated("HELTEC V4"),
        firmware: Text::from_truncated(concat!("PHY ", env!("CARGO_PKG_VERSION"))),
        radio: RadioState::Booting,
        host: HostState::Detached,
        power_source: PowerSource::Unknown,
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

pub fn apply_profile(status: &mut LocalStatus, profile: PhyProfile) {
    status.profile = RadioProfile {
        frequency_hz: Some(profile.frequency_hz),
        bandwidth_hz: Some(profile.bandwidth_hz),
        spreading_factor: Some(profile.spreading_factor),
        coding_rate_denominator: Some(profile.coding_rate_denominator),
        tx_power_dbm: Some(profile.tx_power_dbm),
        sync_word: Some(profile.sync_word),
        name: Text::from_truncated("HOST"),
    };
}
