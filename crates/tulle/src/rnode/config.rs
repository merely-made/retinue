//! RNode carrier configuration, the device's echoes of it, and how its complaints are
//! classified (`RNodeInterface.py` 110-160, 307-349, 650-695, 1076-1097).

use core::fmt;

use super::cmd;
use crate::lora::LoRaParams;

/// The largest DATA payload: a 500-byte packet plus the default 8-byte IFAC
/// (`RNodeInterface.py` 110, 195).
pub const HW_MTU: usize = 508;
/// Frequency range RNS accepts (`RNodeInterface.py` 113-114).
pub const FREQ_MIN: u32 = 137_000_000;
pub const FREQ_MAX: u32 = 3_000_000_000;
/// The oldest firmware RNS drives (`RNodeInterface.py` 119-120).
pub const MIN_FIRMWARE: (u8, u8) = (1, 52);
/// Largest frequency echo error RNS tolerates (`RNodeInterface.py` 678).
pub const FREQ_TOLERANCE_HZ: u32 = 100;

/// One RNode's settings beyond the modulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RNodeConfig {
    pub params: LoRaParams,
    /// Short- and long-term airtime limits in hundredths of a percent (0..=10_000), as RNS
    /// sends `int(percent * 100)` (`RNodeInterface.py` 619-641).
    pub st_alock: Option<u16>,
    pub lt_alock: Option<u16>,
    /// Hold each frame until the device answers READY (`RNodeInterface.py` 711-744).
    pub flow_control: bool,
    /// Firmware below this is refused.
    pub min_firmware: (u8, u8),
}

impl RNodeConfig {
    pub fn new(params: LoRaParams) -> Self {
        Self {
            params,
            st_alock: None,
            lt_alock: None,
            flow_control: false,
            min_firmware: MIN_FIRMWARE,
        }
    }

    /// RNS's range checks (`RNodeInterface.py` 307-334); a coding rate is valid by type.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let p = &self.params;
        let checks = [
            (
                (FREQ_MIN..=FREQ_MAX).contains(&p.frequency_hz),
                ConfigError::Frequency,
            ),
            (p.tx_power_dbm <= 37, ConfigError::TxPower),
            (
                (7_800..=1_625_000).contains(&p.bandwidth_hz),
                ConfigError::Bandwidth,
            ),
            (
                (5..=12).contains(&p.spreading_factor),
                ConfigError::SpreadingFactor,
            ),
            (
                [self.st_alock, self.lt_alock]
                    .iter()
                    .flatten()
                    .all(|&a| a <= 10_000),
                ConfigError::AirtimeLimit,
            ),
        ];
        checks
            .into_iter()
            .find_map(|(ok, error)| (!ok).then_some(error))
            .map_or(Ok(()), Err)
    }
}

/// A setting outside the range RNS accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Frequency,
    TxPower,
    Bandwidth,
    SpreadingFactor,
    AirtimeLimit,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid RNode {self:?} setting")
    }
}

impl std::error::Error for ConfigError {}

/// The settings the device echoed back, recorded as they arrive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reported {
    pub frequency_hz: Option<u32>,
    pub bandwidth_hz: Option<u32>,
    pub tx_power_dbm: Option<u8>,
    pub spreading_factor: Option<u8>,
    pub coding_rate: Option<u8>,
    pub radio_state: Option<u8>,
    pub st_alock: Option<u16>,
    pub lt_alock: Option<u16>,
}

impl Reported {
    /// Record an echo. False if `command` is not a setting.
    pub(super) fn record(&mut self, command: u8, payload: &[u8]) -> bool {
        let byte = payload.first().copied();
        let word = payload
            .get(..4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        let half = payload.get(..2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        match command {
            cmd::FREQUENCY => self.frequency_hz = word,
            cmd::BANDWIDTH => self.bandwidth_hz = word,
            cmd::TXPOWER => self.tx_power_dbm = byte,
            cmd::SF => self.spreading_factor = byte,
            cmd::CR => self.coding_rate = byte,
            cmd::RADIO_STATE => self.radio_state = byte,
            cmd::ST_ALOCK => self.st_alock = half,
            cmd::LT_ALOCK => self.lt_alock = half,
            _ => return false,
        }
        true
    }

    /// The first echo that disagrees with `config`.
    ///
    /// Stricter than RNS (`RNodeInterface.py` 667-695): a missing frequency echo and a wrong
    /// coding rate both fail here. Airtime limits are checked only when echoed, since RNS
    /// records but never compares them.
    pub fn mismatch(&self, config: &RNodeConfig) -> Option<Mismatch> {
        let p = &config.params;
        let alock = |asked: Option<u16>, echoed: Option<u16>| matches!((asked, echoed), (Some(a), Some(e)) if a != e);
        let checks = [
            (
                self.frequency_hz
                    .is_some_and(|f| f.abs_diff(p.frequency_hz) <= FREQ_TOLERANCE_HZ),
                Mismatch::Frequency,
            ),
            (
                self.bandwidth_hz == Some(p.bandwidth_hz),
                Mismatch::Bandwidth,
            ),
            (self.tx_power_dbm == Some(p.tx_power_dbm), Mismatch::TxPower),
            (
                self.spreading_factor == Some(p.spreading_factor),
                Mismatch::SpreadingFactor,
            ),
            (
                self.coding_rate == Some(super::coding_rate_wire(p)),
                Mismatch::CodingRate,
            ),
            (self.radio_state == Some(1), Mismatch::RadioState),
            (
                !alock(config.st_alock, self.st_alock) && !alock(config.lt_alock, self.lt_alock),
                Mismatch::AirtimeLimit,
            ),
        ];
        checks.into_iter().find_map(|(ok, m)| (!ok).then_some(m))
    }
}

/// A setting the device reported differently from what was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mismatch {
    Frequency,
    Bandwidth,
    TxPower,
    SpreadingFactor,
    CodingRate,
    RadioState,
    AirtimeLimit,
}

/// An `ERROR` code, as RNS treats it (`RNodeInterface.py` 1076-1090).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceError {
    /// Radio init or transmit failure, or a code RNS does not know: the carrier restarts.
    Fatal(u8),
    /// Memory exhausted or a modem timeout: recorded, the carrier stays up.
    Recorded(u8),
}

impl DeviceError {
    pub const MEMORY_LOW: u8 = 0x05;
    pub const MODEM_TIMEOUT: u8 = 0x06;

    pub fn classify(code: u8) -> Self {
        match code {
            Self::MEMORY_LOW | Self::MODEM_TIMEOUT => Self::Recorded(code),
            _ => Self::Fatal(code),
        }
    }
}

/// Why an RNode cannot carry traffic until it is reopened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The echoed settings disagree with the configuration.
    Mismatch(Mismatch),
    /// Firmware older than the configured floor, as (major, minor).
    Firmware(u8, u8),
    /// A fatal `ERROR` code.
    Device(u8),
    /// The device reset while online (`RNodeInterface.py` 1091-1097).
    Reset,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mismatch(m) => write!(f, "RNode reported a {m:?} mismatch"),
            Self::Firmware(major, minor) => write!(f, "RNode firmware {major}.{minor} is too old"),
            Self::Device(code) => write!(f, "RNode hardware error 0x{code:02x}"),
            Self::Reset => write!(f, "RNode reset while online"),
        }
    }
}

impl std::error::Error for Fault {}
