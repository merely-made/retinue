#![forbid(unsafe_code)]

//! Postilion: the shared radio-host library of the retinue family.
//!
//! The host-side work every radio-driving application repeats, held once (compare
//! [`outrider`](https://crates.io/crates/outrider), which escorts from alongside).
//!
//! A [`Station`] is one operator on one radio: a caller-supplied identity, a board on a serial
//! port in either personality, an announce cadence, a table of peers heard, and a stream of
//! [`Event`]s. It has no user interface or identity store, so a terminal, a GUI and a test
//! harness can share one implementation.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use postilion::{Event, Radio, Station, StationConfig};
//! use retinue::identity::PrivateIdentity;
//!
//! let identity = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
//! let mut station = Station::open(StationConfig::new("COM6", "alice", identity))
//! .await?;
//!
//! println!("you are {}", station.address());
//! while let Some(event) = station.next_event().await {
//!     if let Event::Message { from, payload, .. } = event {
//!         println!("[{from}] {}", String::from_utf8_lossy(&payload.content));
//!     }
//! }
//! # Ok(()) }
//! ```

pub mod control;
pub mod management;

mod config;
mod error;
mod event;
mod held;
mod station;

pub use config::{DEFAULT_RESOURCE_TIMEOUT, Radio, StationConfig, StationRadioConfig, profile};
pub use error::Error;
pub use event::{Event, Peer, Sent};
pub use station::Station;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use retinue::identity::PrivateIdentity;

    use super::station::radio_resource_config;
    use super::*;

    #[test]
    fn station_config_requires_a_caller_supplied_identity() {
        let identity = PrivateIdentity::from_secret_bytes(&[0x41; 64]);
        let config = StationConfig::new("COM6", "bench", identity.clone());

        assert_eq!(config.port, "COM6");
        assert_eq!(config.name, "bench");
        assert_eq!(config.resource_timeout, DEFAULT_RESOURCE_TIMEOUT);
        assert_eq!(config.identity.public().hash(), identity.public().hash());
    }

    /// Station carriage must use the same strict half-duplex policy as the qualified
    /// two-board Resource receipt. The fast-link default reproduces collisions on hardware.
    #[test]
    fn radio_resource_policy_is_profile_derived_and_single_turn() {
        let params = tulle::lora::LoRaParams::try_from(profile(250_000)).unwrap();
        let timeout = Duration::from_secs(75);
        let config = radio_resource_config(&params, timeout);

        assert_eq!(config.timeout, timeout);
        assert_eq!(
            config.retry_interval,
            tulle::pacing::resource_retry(&params, false)
        );
        assert_eq!(config.request_window, 1);
    }

    #[test]
    fn radio_modes_parse_and_default_to_the_family_protocol() {
        assert_eq!(Radio::parse("phy"), Some(Radio::Phy));
        assert_eq!(Radio::parse("rnode"), Some(Radio::Rnode));
        assert_eq!(Radio::parse("meshtastic"), None);
        assert_eq!(Radio::default(), Radio::Phy);
    }

    /// The on-air settings a host protocol cannot reach are this family's own, and both
    /// personalities must agree on them or two of our own boards cannot hear each other.
    #[test]
    fn the_trunk_profile_is_the_familys_own_air() {
        let profile = profile(250_000);
        assert_eq!(profile.sync_word, 0x12);
        assert_eq!(profile.preamble_symbols, 16);
        assert_eq!(profile.spreading_factor, 8);
        assert_eq!(profile.coding_rate_denominator, 5);
        assert!(profile.explicit_header && profile.crc && !profile.invert_iq);
    }
}
