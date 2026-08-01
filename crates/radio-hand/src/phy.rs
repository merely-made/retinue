//! Wire values to SX126x modulation parameters.
//!
//! `selvage` owns the profile as data: a spreading factor is a `u8`, a bandwidth is a `u32`
//! of hertz, a coding rate is its denominator. Driving a radio means turning those into
//! `lora-modulation`'s enums, and rejecting values no SX126x accepts.
//!
//! This lived twice, byte for byte, in the two firmware `main.rs` files. It is board-agnostic
//! (both boards carry an SX1262) so it belongs here rather than in either image, and it stays
//! out of `selvage` so the host side never inherits a radio-driver dependency.

use lora_modulation::{Bandwidth, CodingRate, SpreadingFactor};

/// The spreading factor for a wire value, or `None` if the SX126x has no such setting.
pub fn spreading_factor(value: u8) -> Option<SpreadingFactor> {
    Some(match value {
        5 => SpreadingFactor::_5,
        6 => SpreadingFactor::_6,
        7 => SpreadingFactor::_7,
        8 => SpreadingFactor::_8,
        9 => SpreadingFactor::_9,
        10 => SpreadingFactor::_10,
        11 => SpreadingFactor::_11,
        12 => SpreadingFactor::_12,
        _ => return None,
    })
}

/// The bandwidth for a wire value in hertz, or `None` if it is not one the SX126x offers.
///
/// The values are the chip's own, which is why they are exact rather than ranges: 7.81 kHz
/// and its doublings, as the datasheet lists them.
pub fn bandwidth(value: u32) -> Option<Bandwidth> {
    Some(match value {
        7_810 => Bandwidth::_7KHz,
        10_420 => Bandwidth::_10KHz,
        15_630 => Bandwidth::_15KHz,
        20_830 => Bandwidth::_20KHz,
        31_250 => Bandwidth::_31KHz,
        41_670 => Bandwidth::_41KHz,
        62_500 => Bandwidth::_62KHz,
        125_000 => Bandwidth::_125KHz,
        250_000 => Bandwidth::_250KHz,
        500_000 => Bandwidth::_500KHz,
        _ => return None,
    })
}

/// The coding rate for a wire denominator (`4/N`), or `None` outside 5..=8.
pub fn coding_rate(value: u8) -> Option<CodingRate> {
    Some(match value {
        5 => CodingRate::_4_5,
        6 => CodingRate::_4_6,
        7 => CodingRate::_4_7,
        8 => CodingRate::_4_8,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spreading_factor_the_chip_has_decodes_and_nothing_else_does() {
        for sf in 5..=12u8 {
            assert!(spreading_factor(sf).is_some(), "SF{sf} is a real setting");
        }
        for sf in [0, 1, 4, 13, 255] {
            assert!(spreading_factor(sf).is_none(), "SF{sf} is not");
        }
    }

    #[test]
    fn coding_rate_takes_denominators_and_refuses_the_rest() {
        for cr in 5..=8u8 {
            assert!(coding_rate(cr).is_some(), "4/{cr} is a real setting");
        }
        for cr in [0, 1, 4, 9, 255] {
            assert!(coding_rate(cr).is_none(), "4/{cr} is not");
        }
    }

    #[test]
    fn bandwidth_is_exact_not_nearest() {
        // A profile carrying a plausible-but-wrong bandwidth must be refused rather than
        // silently rounded, or two nodes would disagree about the channel while both
        // believing they had configured it.
        assert!(bandwidth(250_000).is_some());
        assert!(bandwidth(249_999).is_none());
        assert!(bandwidth(250_001).is_none());
        assert!(bandwidth(0).is_none());
    }

    #[test]
    fn the_longfast_profile_decodes_end_to_end() {
        // The profile both boards ship with, and the one the RF receipts were taken on.
        assert!(spreading_factor(11).is_some());
        assert!(bandwidth(250_000).is_some());
        assert!(coding_rate(5).is_some());
    }
}
