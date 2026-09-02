//! Sustained post-boot physical presence on the Heltec V4 user button.
//!
//! GPIO0 is a boot strap.  A low level at reset is therefore deliberately
//! insufficient: this observer first sees a released level, then a later
//! falling edge, then an uninterrupted low hold.  The resulting token is
//! private, non-`Copy`, and consumed by the commissioning carrier.

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::gpio::Input;

/// Total time after board startup in which a physical claim gesture is heard.
pub(crate) const PRESENCE_OBSERVATION_WINDOW: Duration = Duration::from_secs(20);
/// Low time required after a fresh falling edge to witness physical presence.
pub(crate) const PRESENCE_HOLD_DURATION: Duration = Duration::from_secs(3);

/// Private proof of one sustained post-boot physical gesture.
///
/// Only this module can mint it, and the commissioning session consumes it.
pub(crate) struct PhysicalPresence {
    _private: (),
}

/// State model for the GPIO0 gesture.  Keeping it pure makes the bootstrap-low
/// rule and bounce handling testable without ESP GPIO hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WitnessState {
    AwaitRelease,
    AwaitFalling,
    HoldingLow,
}

impl WitnessState {
    const fn from_level_is_high(is_high: bool) -> Self {
        if is_high {
            Self::AwaitFalling
        } else {
            Self::AwaitRelease
        }
    }

    const fn rising(self) -> Self {
        match self {
            Self::AwaitRelease | Self::HoldingLow => Self::AwaitFalling,
            Self::AwaitFalling => Self::AwaitFalling,
        }
    }

    const fn falling(self) -> Self {
        match self {
            Self::AwaitFalling => Self::HoldingLow,
            Self::AwaitRelease | Self::HoldingLow => self,
        }
    }

    const fn held_low(self) -> bool {
        matches!(self, Self::HoldingLow)
    }
}

/// Wait for one entire release, press, and hold sequence while retaining GPIO
/// custody for ordinary UI if the observation window expires.
pub(crate) async fn observe(
    mut button: Input<'static>,
) -> (Input<'static>, Option<PhysicalPresence>) {
    let observed = with_timeout(PRESENCE_OBSERVATION_WINDOW, observe_gesture(&mut button)).await;
    let token = match observed {
        Ok(()) => Some(PhysicalPresence { _private: () }),
        Err(_) => None,
    };
    (button, token)
}

async fn observe_gesture(button: &mut Input<'static>) {
    let mut state = WitnessState::from_level_is_high(button.is_high());
    loop {
        match state {
            WitnessState::AwaitRelease => {
                button.wait_for_rising_edge().await;
                state = state.rising();
            }
            WitnessState::AwaitFalling => {
                button.wait_for_falling_edge().await;
                state = state.falling();
            }
            WitnessState::HoldingLow => {
                match select(
                    button.wait_for_rising_edge(),
                    Timer::after(PRESENCE_HOLD_DURATION),
                )
                .await
                {
                    Either::First(()) => state = state.rising(),
                    Either::Second(()) if state.held_low() && button.is_low() => return,
                    Either::Second(()) => {
                        state = WitnessState::from_level_is_high(button.is_high())
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_low_requires_a_release_then_a_new_falling_edge() {
        let state = WitnessState::from_level_is_high(false);
        assert_eq!(state, WitnessState::AwaitRelease);
        assert!(!state.held_low());
        let released = state.rising();
        assert_eq!(released, WitnessState::AwaitFalling);
        let pressed = released.falling();
        assert_eq!(pressed, WitnessState::HoldingLow);
        assert!(pressed.held_low());
    }

    #[test]
    fn bounce_restarts_the_hold_from_a_later_falling_edge() {
        let holding = WitnessState::from_level_is_high(true).falling();
        assert_eq!(holding, WitnessState::HoldingLow);
        let released = holding.rising();
        assert_eq!(released, WitnessState::AwaitFalling);
        assert!(!released.held_low());
        assert_eq!(released.falling(), WitnessState::HoldingLow);
    }
}
