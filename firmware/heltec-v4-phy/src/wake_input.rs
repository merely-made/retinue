//! ESP32-S3 GPIO wait adapter that preserves DIO1 as a Light-sleep wake source.
//!
//! ESP-HAL deliberately separates the global [`GpioWakeupSource`] from each pin's
//! `wakeup_enable` bit. Its async `wait_for_high` configures the CPU interrupt and waker, but
//! rewrites the pin register with wake disabled. DIO1 needs both at once: the level must wake
//! the CPU and the GPIO interrupt must resume the LoRa IRQ waiter and receive path.
//!
//! [`GpioWakeupSource`]: esp_hal::rtc_cntl::sleep::GpioWakeupSource

use core::convert::Infallible;
use core::future::{Future, poll_fn};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::Poll;

use embedded_hal::digital::ErrorType;
use embedded_hal_async::digital::Wait;
use esp_hal::gpio::{Input, WakeEvent};

use crate::wake_lease::{WakeRegister, arm_and_check};

const DIO1_GPIO: usize = 14;
static RADIO_WAKE_ARMED: AtomicBool = AtomicBool::new(false);
static RADIO_WAKE_REGISTRATIONS: AtomicU32 = AtomicU32::new(0);

/// The only writer for DIO1's Light-sleep wake bit.
///
/// It has no borrow of [`Input`], so a [`crate::wake_lease::WakeLease`] can clean it up after the
/// inner ESP-HAL wait future has been dropped. ESP-HAL's future then retains responsibility for
/// unlistening and clearing the pending GPIO interrupt; the lease only owns the extra wake bit
/// this adapter adds after ESP-HAL has registered its waiter.
///
/// On this board the SX1262 holds DIO1 high until the LoRa owner clears its IRQ flags. That owner
/// is exclusive while this waiter is being established, so a high read immediately after the raw
/// wake-bit RMW means an interrupt may have completed just before that RMW. The same poll returns
/// ready in that case, which drops both guards and reaches the normal locked cleanup without
/// sleeping on a stale registration.
struct Dio1WakeRegister;

impl WakeRegister for Dio1WakeRegister {
    fn set_wake(&mut self, enabled: bool) {
        // SAFETY: this does not claim ownership of GPIO or modify any field owned by another
        // pin. GPIO14 is already exclusively owned by this adapter; the volatile PAC operation
        // changes only its Light-sleep wake-enable bit and leaves the CPU interrupt intact.
        unsafe {
            (*esp32s3::GPIO::ptr())
                .pin(DIO1_GPIO)
                .modify(|_, w| w.wakeup_enable().bit(enabled));
        }
        if enabled {
            RADIO_WAKE_REGISTRATIONS.fetch_add(1, Ordering::Relaxed);
        }
        RADIO_WAKE_ARMED.store(enabled, Ordering::Release);
    }
}

pub fn radio_wake_armed() -> bool {
    RADIO_WAKE_ARMED.load(Ordering::Acquire)
}

pub fn radio_is_high() -> bool {
    // SAFETY: read-only access to the bank-0 GPIO input snapshot.
    unsafe { (*esp32s3::GPIO::ptr()).in_().read().data_next().bits() & (1 << DIO1_GPIO) != 0 }
}

pub fn radio_wake_registrations() -> u32 {
    RADIO_WAKE_REGISTRATIONS.load(Ordering::Relaxed)
}

pub struct V4Input {
    input: Input<'static>,
    wake_on_high: bool,
}

impl V4Input {
    pub fn new(input: Input<'static>, wake_on_high: bool) -> Self {
        Self {
            input,
            wake_on_high,
        }
    }
}

impl ErrorType for V4Input {
    type Error = Infallible;
}

impl Wait for V4Input {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        if !self.wake_on_high {
            self.input.wait_for_high().await;
            return Ok(());
        }

        // The first poll registers ESP-HAL's normal GPIO interrupt and Embassy waker. Only
        // after that registration may the wake bit be set, because `wait_for_high` otherwise
        // overwrites it. The PAC write changes that one bit and leaves the high-level CPU
        // interrupt intact.
        let mut wake_lease = None;
        {
            let mut wait = core::pin::pin!(self.input.wait_for_high());
            poll_fn(|cx| match wait.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(()),
                Poll::Pending => {
                    if wake_lease.is_none() {
                        // `wake_lease` outlives the nested ESP-HAL future. If our caller drops
                        // this wait after registration, ESP-HAL first unlistens and clears the
                        // pending interrupt, then the lease disables the extra Light-sleep wake
                        // bit and clears the public armed state.
                        let (lease, high) = arm_and_check(Dio1WakeRegister, radio_is_high);
                        wake_lease = Some(lease);
                        if high {
                            return Poll::Ready(());
                        }
                    }
                    Poll::Pending
                }
            })
            .await;
        }
        drop(wake_lease);
        // The S3's GPIO wake enable is level-sensitive and persists after wake. Tear it down
        // before the SX1262 IRQ is processed and later re-armed, otherwise a stale high/status
        // can make the following Light-sleep request return immediately.
        let _ = self.input.wakeup_enable(false, WakeEvent::HighLevel);
        self.input.clear_interrupt();
        RADIO_WAKE_ARMED.store(false, Ordering::Release);
        Ok(())
    }

    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        self.input.wait_for_low().await;
        Ok(())
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        self.input.wait_for_rising_edge().await;
        Ok(())
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        self.input.wait_for_falling_edge().await;
        Ok(())
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        self.input.wait_for_any_edge().await;
        Ok(())
    }
}
