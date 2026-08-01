//! Projecting a radio profile onto the local face.
//!
//! A third function both images carried identically: when a host applies a profile, the
//! face should show what the radio is actually set to, labelled as the host's doing rather
//! than the board's boot default.

use radio_face::{LocalStatus, RadioProfile, Text};
use selvage::PhyProfile;

/// Show an applied host profile on the local face.
///
/// The name is `HOST` rather than the profile's own, because at this point the board is no
/// longer on the preset it booted with and saying otherwise would be a face that lies.
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
