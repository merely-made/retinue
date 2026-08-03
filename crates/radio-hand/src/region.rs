//! The regulatory floor: compliant region profiles, as data.
//!
//! Pressure point 1's ruling, in Mark's shape: do what Meshtastic and the others do. A
//! built-in table of region profiles under plain names, each supplying frequency bounds, a
//! power cap, and a duty limit. The user picks one at first setup; the choice persists as a
//! board fact; every profile is validated against it, and **until a region is chosen the
//! board does not transmit** — the honest posture, per the no-placebo rule.
//!
//! The table is data: adding a region is an entry, not a code path. The values follow the
//! shape of the Meshtastic region table and the common ISM allocations; **verify an entry
//! against the current national rules before shipping boards into that region** — that
//! review is exactly why this is a table.
//!
//! Power is clamped, never rejected: a profile may ask for less than the cap and gets what
//! it asked; asking for more gets the cap, and the *applied* value is what the status
//! reports. Frequency is rejected, never bent: silently retuning a request is worse than
//! refusing it.

/// A compliance profile the user picks from.
///
/// `Unset` is the state a board ships in and the state an existing record upgrades into
/// (the settings byte was reserved-as-zero), and it means what it says: no transmit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Region {
    /// No region chosen. The board receives but does not transmit.
    #[default]
    Unset = 0,
    /// United States, 902–928 MHz (FCC part 15.247).
    Us915 = 1,
    /// European Union, the 869.40–869.65 MHz sub-band (10 % duty, 500 mW ERP).
    Eu868 = 2,
    /// European Union, 433.05–434.79 MHz (10 % duty, low power).
    Eu433 = 3,
    /// Australia and New Zealand, 915–928 MHz.
    Anz915 = 4,
    /// Japan, 920.6–923.4 MHz (ARIB STD-T108).
    Jp920 = 5,
}

/// What a region permits. All limits inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionProfile {
    /// The plain name the face and probes show.
    pub name: &'static str,
    pub freq_min_hz: u32,
    pub freq_max_hz: u32,
    /// Conducted-power cap in dBm. The hardware's own ceiling (the SX1262's +22) applies
    /// on top; the applied power is the minimum of request, region, and hardware.
    pub max_power_dbm: i8,
    /// Transmit duty limit in permille per hour. Zero means the region imposes none.
    pub duty_permille: u16,
    /// Whether listen-before-talk is a *requirement* here rather than good manners.
    ///
    /// The distinction decides what happens when the channel stays busy: where deference
    /// is mandatory the frame is refused, and where it is courtesy the board takes its turn
    /// after a bounded wait. Japan's ARIB STD-T108 mandates carrier sense; the FCC part
    /// 15.247 digital-modulation rules the US entry follows do not.
    pub listen_required: bool,
    /// The trunk's default carrier here, used for the boot profile. Our choice within the
    /// band, not a regulatory value.
    pub default_frequency_hz: u32,
}

/// The compliance table. Indexed by [`Region`]; `Unset` deliberately has no entry.
const TABLE: &[(Region, RegionProfile)] = &[
    (
        Region::Us915,
        RegionProfile {
            name: "US915",
            freq_min_hz: 902_000_000,
            freq_max_hz: 928_000_000,
            max_power_dbm: 30,
            duty_permille: 0,
            listen_required: false,
            default_frequency_hz: 906_875_000,
        },
    ),
    (
        Region::Eu868,
        RegionProfile {
            name: "EU868",
            freq_min_hz: 869_400_000,
            freq_max_hz: 869_650_000,
            max_power_dbm: 27,
            duty_permille: 100,
            listen_required: false,
            default_frequency_hz: 869_525_000,
        },
    ),
    (
        Region::Eu433,
        RegionProfile {
            name: "EU433",
            freq_min_hz: 433_050_000,
            freq_max_hz: 434_790_000,
            max_power_dbm: 12,
            duty_permille: 100,
            listen_required: false,
            default_frequency_hz: 433_875_000,
        },
    ),
    (
        Region::Anz915,
        RegionProfile {
            name: "ANZ915",
            freq_min_hz: 915_000_000,
            freq_max_hz: 928_000_000,
            max_power_dbm: 30,
            duty_permille: 0,
            listen_required: false,
            default_frequency_hz: 916_875_000,
        },
    ),
    (
        Region::Jp920,
        RegionProfile {
            name: "JP920",
            freq_min_hz: 920_600_000,
            freq_max_hz: 923_400_000,
            max_power_dbm: 13,
            duty_permille: 100,
            listen_required: true,
            default_frequency_hz: 921_875_000,
        },
    ),
];

impl Region {
    /// The region a stored byte names, or `None` for a byte this build does not know.
    ///
    /// Unknown is refused rather than guessed, same posture as the channel byte: a board
    /// that downgraded past a region it once ran must fall back to not transmitting, never
    /// to someone else's rules.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Region::Unset),
            1 => Some(Region::Us915),
            2 => Some(Region::Eu868),
            3 => Some(Region::Eu433),
            4 => Some(Region::Anz915),
            5 => Some(Region::Jp920),
            _ => None,
        }
    }

    pub fn as_byte(self) -> u8 {
        self as u8
    }

    /// The compliance profile, or `None` for `Unset` — which is what makes "no region, no
    /// transmit" fall out of the type rather than out of a flag.
    pub fn profile(self) -> Option<&'static RegionProfile> {
        TABLE
            .iter()
            .find(|(region, _)| *region == self)
            .map(|(_, profile)| profile)
    }

    /// The plain name, for probes and the face.
    pub fn name(self) -> &'static str {
        self.profile().map(|p| p.name).unwrap_or("unset")
    }

    /// Every region a user may pick, for a setup surface to list.
    pub fn choices() -> impl Iterator<Item = Region> {
        TABLE.iter().map(|(region, _)| *region)
    }
}

impl RegionProfile {
    /// Whether `frequency_hz` lies inside this region's band.
    pub fn allows_frequency(&self, frequency_hz: u32) -> bool {
        (self.freq_min_hz..=self.freq_max_hz).contains(&frequency_hz)
    }

    /// The power actually applied for a request: the minimum of the request, this region's
    /// cap, and the hardware's ceiling.
    pub fn clamp_power(&self, requested_dbm: i8, hardware_max_dbm: i8) -> i8 {
        requested_dbm.min(self.max_power_dbm).min(hardware_max_dbm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_region_round_trips_and_has_a_profile() {
        for region in Region::choices() {
            assert_eq!(Region::from_byte(region.as_byte()), Some(region));
            let profile = region.profile().expect("every choice has a profile");
            assert!(
                profile.allows_frequency(profile.default_frequency_hz),
                "{}: the trunk default must lie inside its own band",
                profile.name
            );
            assert!(profile.freq_min_hz < profile.freq_max_hz);
        }
    }

    #[test]
    fn unset_is_zero_has_no_profile_and_does_not_name_a_place() {
        // Zero, because the settings byte it decodes from was reserved-as-zero: every
        // record written before regions existed upgrades into Unset, never into a region.
        assert_eq!(Region::Unset.as_byte(), 0);
        assert_eq!(Region::from_byte(0), Some(Region::Unset));
        assert!(Region::Unset.profile().is_none());
        assert_eq!(Region::Unset.name(), "unset");
    }

    #[test]
    fn an_unknown_byte_is_refused() {
        assert_eq!(Region::from_byte(0xEE), None);
    }

    #[test]
    fn power_clamps_to_region_then_hardware() {
        let us = Region::Us915.profile().unwrap();
        // The region would allow 30, the SX1262 cannot: hardware wins.
        assert_eq!(us.clamp_power(30, 22), 22);
        // A modest request passes through untouched.
        assert_eq!(us.clamp_power(17, 22), 17);

        let jp = Region::Jp920.profile().unwrap();
        // The region caps below the hardware: region wins.
        assert_eq!(jp.clamp_power(22, 22), 13);
    }

    #[test]
    fn frequency_is_a_hard_bound() {
        let eu = Region::Eu868.profile().unwrap();
        assert!(eu.allows_frequency(869_525_000));
        assert!(!eu.allows_frequency(868_100_000), "below the sub-band");
        assert!(!eu.allows_frequency(906_875_000), "a US carrier");
    }
}
