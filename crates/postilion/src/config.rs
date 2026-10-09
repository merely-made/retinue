//! Station configuration and this family's trunk radio profile.

use std::time::Duration;

use retinue::identity::PrivateIdentity;
use tulle::PhyProfile;

use crate::management::DEFAULT_ANNOUNCE_HISTORY_BOUND;

/// Qualified whole-transfer deadline for a station's direct radio carriage.
///
/// The two-board Resource receipt uses this deadline. Callers can narrow or widen it through
/// [`StationConfig::resource_timeout`] without replacing the profile-derived retry policy.
pub const DEFAULT_RESOURCE_TIMEOUT: Duration = Duration::from_secs(120);

/// Which board personality is on the other end of the cable.
///
/// The same PHY either way: the RNode channel programs the sync word and preamble every other
/// personality in this family uses, so the two are on one another's air. What differs is only
/// which host protocol the board speaks, which is why one library serves both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Radio {
    /// The direct-PHY modem, this family's own host protocol.
    #[default]
    Phy,
    /// The RNode channel, the protocol stock Reticulum clients speak.
    Rnode,
}

impl Radio {
    /// Parse a mode name, for a command line or a config file.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "phy" => Some(Radio::Phy),
            "rnode" => Some(Radio::Rnode),
            _ => None,
        }
    }
}

/// How a station is brought up.
#[derive(Clone, Debug)]
pub struct StationConfig {
    /// Serial port the board is on.
    pub port: String,
    /// Display name, carried in the delivery announce and used as a message title.
    pub name: String,
    /// Channel bandwidth. The rest of the profile is this family's trunk shape.
    pub bandwidth_hz: u32,
    /// Which host protocol the board speaks.
    pub radio: Radio,
    /// How often to re-announce. A receiver cannot verify an identity it has never heard
    /// announce; the 30 s default suits a small group, not a shared band.
    pub announce_interval: Duration,
    /// Maximum announce observations retained for management history.
    pub announce_history_bound: usize,
    /// Maximum time allowed for one complete direct-message Resource transfer.
    ///
    /// Retries and request turns are derived from the selected radio profile; this is the
    /// owner-controlled failure horizon around that mechanism.
    pub resource_timeout: Duration,
    /// The station's Reticulum identity, supplied by the host. Postilion neither persists nor
    /// creates one: the application owns its credential boundary.
    pub identity: PrivateIdentity,
}

impl StationConfig {
    /// Build a station configuration with this family's ordinary radio defaults.
    pub fn new(
        port: impl Into<String>,
        name: impl Into<String>,
        identity: PrivateIdentity,
    ) -> Self {
        Self {
            port: port.into(),
            name: name.into(),
            bandwidth_hz: 250_000,
            radio: Radio::Phy,
            announce_interval: Duration::from_secs(30),
            announce_history_bound: DEFAULT_ANNOUNCE_HISTORY_BOUND,
            resource_timeout: DEFAULT_RESOURCE_TIMEOUT,
            identity,
        }
    }
}

/// Non-secret radio configuration retained for management snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationRadioConfig {
    pub port: String,
    pub bandwidth_hz: u32,
    pub radio: Radio,
    pub announce_interval: Duration,
    pub announce_history_bound: usize,
}

impl From<&StationConfig> for StationRadioConfig {
    fn from(config: &StationConfig) -> Self {
        Self {
            port: config.port.clone(),
            bandwidth_hz: config.bandwidth_hz,
            radio: config.radio,
            announce_interval: config.announce_interval,
            announce_history_bound: config.announce_history_bound,
        }
    }
}

/// This family's trunk profile at a chosen bandwidth.
pub fn profile(bandwidth_hz: u32) -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz,
        spreading_factor: 8,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: 0x12,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 17,
    }
}
