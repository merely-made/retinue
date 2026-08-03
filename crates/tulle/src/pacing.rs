//! Retry floors derived from what the medium actually costs.
//!
//! Three separate defects in this project traced to the same root: a retry interval picked
//! as a constant, tuned for links far faster than LoRa.
//!
//! - **N4, link setup.** `DEFAULT_LINK_SETUP_RETRY_MS` is 2000. At SF11/250 kHz a link
//!   request is ~0.9 s of air and the proof ~1.4 s more, so every retry fired straight into
//!   the proof it was waiting for — while the half-duplex board missed the retry because it
//!   was still transmitting the answer. Both sides lost in perfect synchrony, run after run.
//! - **N5, resource transfer.** The same shape one layer up: a request and the part that
//!   answers it must both clear before the next request is due.
//! - **Pressure point 2, listen-before-talk.** Carrier sense adds ~66 ms per frame plus
//!   randomised backoff. Small in isolation, and enough to desync a request/response loop
//!   whose intervals had no margin for it — measured as 3 of 8 against 8 of 8.
//!
//! Each was found on hardware and fixed by hand-picking a bigger number. This computes them
//! instead, from the profile's own time on air, so a slower spreading factor or a wider
//! payload moves the floors with it.
//!
//! These are **floors, not schedules**. A caller may wait longer for its own reasons; it
//! must not wait less, because less means retransmitting into the answer.

use core::time::Duration;

use crate::lora::LoRaParams;

/// The margin every floor carries over its computed round trip.
///
/// Covers what the airtime formula does not: the board's own processing between receive and
/// reply, serial transport to a host-driven modem, and scheduler slack. Twenty-five percent
/// with a floor of 250 ms — proportional so it grows with slow profiles, floored so fast
/// ones keep a usable margin.
const MARGIN_NUMERATOR: u32 = 5;
const MARGIN_DENOMINATOR: u32 = 4;
const MARGIN_FLOOR: Duration = Duration::from_millis(250);

/// Time on air for a frame of `payload_len` bytes, plus the carrier-sense cost a polite
/// transmitter pays before it.
///
/// `listen_first` accounts for pressure point 2: eight CAD symbols at this profile, plus a
/// conservative allowance for the randomised backoff between deferrals.
fn frame_cost(params: &LoRaParams, payload_len: usize, listen_first: bool) -> Duration {
    let air = params.time_on_air(payload_len);
    if !listen_first {
        return air;
    }
    // Eight symbols of carrier sense, the value lora-phy programs into the SX126x, plus one
    // typical backoff. Not the worst case: a floor that assumed every frame exhausts the
    // whole courtesy budget would be pessimistic enough to slow the common case badly.
    let symbol = params
        .time_on_air(0)
        .div_f64(f64::from(params.preamble_syms) + 4.25);
    air + symbol * 8 + Duration::from_millis(60)
}

/// Apply the standard margin.
fn with_margin(round_trip: Duration) -> Duration {
    let padded = round_trip * MARGIN_NUMERATOR / MARGIN_DENOMINATOR;
    if padded < MARGIN_FLOOR {
        MARGIN_FLOOR
    } else {
        padded
    }
}

/// The shortest sane interval between link-setup retries.
///
/// A link request out and its proof back. Retrying sooner means transmitting into the proof.
pub fn link_setup_retry(params: &LoRaParams, listen_first: bool) -> Duration {
    // A link request is 64 bytes of keys plus a trailer and header; the proof is a signature
    // and its framing. Both sit comfortably under 128 bytes, which is what is costed here
    // rather than the 255-byte maximum: floors should reflect the traffic, not the ceiling.
    let request = frame_cost(params, 128, listen_first);
    let proof = frame_cost(params, 128, listen_first);
    with_margin(request + proof)
}

/// The shortest sane interval between resource-transfer retries.
///
/// A request out and a full part back, since a part is what a request asks for and a part is
/// the largest thing the answer can be.
pub fn resource_retry(params: &LoRaParams, listen_first: bool) -> Duration {
    let request = frame_cost(params, 64, listen_first);
    let part = frame_cost(params, 255, listen_first);
    with_margin(request + part)
}

/// A whole-transfer deadline for `payload_len` bytes at `request_window` parts per turn.
///
/// Not a floor but a budget: every part carried once, plus the retry floor for each turn, so
/// a transfer that loses a frame or two still has room to recover rather than timing out
/// mid-recovery.
pub fn transfer_timeout(
    params: &LoRaParams,
    payload_len: usize,
    request_window: usize,
    listen_first: bool,
) -> Duration {
    let part_bytes = 200_usize;
    let parts = payload_len.div_ceil(part_bytes).max(1);
    let turns = parts.div_ceil(request_window.max(1));
    let carriage = frame_cost(params, part_bytes, listen_first) * parts as u32;
    let turn_overhead = resource_retry(params, listen_first) * turns as u32;
    carriage + turn_overhead
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhyProfile;

    fn long_fast() -> LoRaParams {
        LoRaParams::try_from(PhyProfile::meshtastic_long_fast(906_875_000)).unwrap()
    }

    fn fast() -> LoRaParams {
        let mut profile = PhyProfile::meshtastic_long_fast(906_875_000);
        profile.spreading_factor = 7;
        LoRaParams::try_from(profile).unwrap()
    }

    /// The floor must exceed the constant that was measured failing at N4, which is the
    /// whole reason this module exists.
    #[test]
    fn the_link_floor_beats_the_constant_that_failed_on_hardware() {
        let derived = link_setup_retry(&long_fast(), false);
        assert!(
            derived > Duration::from_millis(2_000),
            "derived {derived:?} must exceed the 2 s default that collided with its own proof"
        );
    }

    /// A slower profile costs more air, so its floor must be larger. This is the property a
    /// hand-picked constant cannot have.
    #[test]
    fn slower_profiles_get_longer_floors() {
        assert!(link_setup_retry(&long_fast(), false) > link_setup_retry(&fast(), false));
        assert!(resource_retry(&long_fast(), false) > resource_retry(&fast(), false));
    }

    /// Carrier sense is not free, and the floor says so.
    #[test]
    fn listening_first_widens_every_floor() {
        let params = long_fast();
        assert!(link_setup_retry(&params, true) > link_setup_retry(&params, false));
        assert!(resource_retry(&params, true) > resource_retry(&params, false));
    }

    /// A resource retry must clear a request plus a full part, or it retransmits into the
    /// part it asked for — N5's failure.
    #[test]
    fn the_resource_floor_clears_a_request_and_a_full_part() {
        let params = long_fast();
        let round_trip = params.time_on_air(64) + params.time_on_air(255);
        assert!(resource_retry(&params, false) > round_trip);
    }

    /// Every floor keeps a usable margin even where the air is nearly free.
    #[test]
    fn fast_profiles_still_get_the_floor_margin() {
        assert!(link_setup_retry(&fast(), false) >= MARGIN_FLOOR);
        assert!(resource_retry(&fast(), false) >= MARGIN_FLOOR);
    }

    /// The derived figures at the trunk profile, printed so a bench can see what it is
    /// being asked to wait for. Run with `--nocapture`.
    #[test]
    fn show_the_derived_floors() {
        for (label, params) in [("SF11/250k", long_fast()), ("SF7/250k", fast())] {
            for listen in [false, true] {
                println!(
                    "{label} listen={listen}: link={:?} resource={:?} timeout(4k,w1)={:?}",
                    link_setup_retry(&params, listen),
                    resource_retry(&params, listen),
                    transfer_timeout(&params, 4_096, 1, listen),
                );
            }
        }
    }

    /// A bigger payload gets a longer deadline, and a wider request window a shorter one.
    #[test]
    fn transfer_timeout_scales_with_payload_and_window() {
        let params = long_fast();
        assert!(
            transfer_timeout(&params, 4_096, 1, false) > transfer_timeout(&params, 1_024, 1, false)
        );
        assert!(
            transfer_timeout(&params, 4_096, 4, false) < transfer_timeout(&params, 4_096, 1, false)
        );
    }
}
