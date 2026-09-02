//! Small cancellation-safe ownership guard for a GPIO wake registration.
//!
//! This stays independent of ESP-HAL so its drop behaviour has a host-testable
//! receipt. The board adapter supplies the one operation that changes hardware.

pub(crate) trait WakeRegister {
    fn set_wake(&mut self, enabled: bool);
}

/// Owns one wake registration until completion or future cancellation.
pub(crate) struct WakeLease<W: WakeRegister> {
    register: W,
    active: bool,
}

impl<W: WakeRegister> WakeLease<W> {
    pub(crate) fn arm(mut register: W) -> Self {
        register.set_wake(true);
        Self {
            register,
            active: true,
        }
    }

    #[cfg(test)]
    fn active(&self) -> bool {
        self.active
    }
}

/// Arms a wake source, then reads its level before yielding to the executor.
///
/// The caller supplies the level read because its GPIO ownership is separate from the raw wake
/// register write. There is intentionally no await between the two operations: a high level
/// means an interrupt could have been serviced immediately before the raw register RMW, so the
/// caller must complete the wait and let the lease disarm rather than parking on a stale setup.
pub(crate) fn arm_and_check<W: WakeRegister>(
    register: W,
    is_high: impl FnOnce() -> bool,
) -> (WakeLease<W>, bool) {
    let lease = WakeLease::arm(register);
    let high = is_high();
    (lease, high)
}

impl<W: WakeRegister> Drop for WakeLease<W> {
    fn drop(&mut self) {
        if self.active {
            self.register.set_wake(false);
            self.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use core::cell::Cell;

    struct Recorder<'a> {
        transitions: &'a Cell<[bool; 2]>,
        len: &'a Cell<usize>,
    }

    impl WakeRegister for Recorder<'_> {
        fn set_wake(&mut self, enabled: bool) {
            let len = self.len.get();
            let mut transitions = self.transitions.get();
            transitions[len] = enabled;
            self.transitions.set(transitions);
            self.len.set(len + 1);
        }
    }

    #[test]
    fn dropped_lease_disarms_a_registered_wake_source() {
        let transitions = Cell::new([false; 2]);
        let len = Cell::new(0);
        {
            let lease = WakeLease::arm(Recorder {
                transitions: &transitions,
                len: &len,
            });
            assert!(lease.active());
        }
        assert_eq!(len.get(), 2);
        assert_eq!(transitions.get(), [true, false]);
    }

    #[test]
    fn stale_rmw_after_an_isr_is_finished_by_the_high_level_handshake() {
        let transitions = Cell::new([false; 2]);
        let len = Cell::new(0);
        // Model the IRQ handler having already cleared its pending status while the SX1262 DIO1
        // line remains high. The wake-register RMW must not turn that past interrupt into a
        // permanently armed sleep source: the same-poll level check completes the wait.
        let irq_status_cleared = Cell::new(true);
        let (lease, high) = arm_and_check(
            Recorder {
                transitions: &transitions,
                len: &len,
            },
            || {
                assert!(irq_status_cleared.get());
                true
            },
        );
        assert!(high);
        drop(lease);
        assert_eq!(len.get(), 2);
        assert_eq!(transitions.get(), [true, false]);
    }
}
