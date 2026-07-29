//! ESP32-S3 GPIO wait adapter that preserves DIO1 as a Light-sleep wake source.
//!
//! ESP-HAL deliberately separates the global [`GpioWakeupSource`] from each pin's
//! `wakeup_enable` bit. Its async `wait_for_high` configures the CPU interrupt and waker, but
//! rewrites the pin register with wake disabled. DIO1 needs both at once: the level must wake
//! the CPU and the GPIO interrupt must resume `lora.rx()`.
//!
//! [`GpioWakeupSource`]: esp_hal::rtc_cntl::sleep::GpioWakeupSource

use core::convert::Infallible;
use core::future::{Future, poll_fn};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::Poll;

use embedded_hal::digital::ErrorType;
use embedded_hal_async::digital::Wait;
use esp_hal::gpio::Input;

const DIO1_GPIO: usize = 14;
static RADIO_WAKE_ARMED: AtomicBool = AtomicBool::new(false);
static RADIO_WAKE_REGISTRATIONS: AtomicU32 = AtomicU32::new(0);

pub fn radio_wake_armed() -> bool {
    RADIO_WAKE_ARMED.load(Ordering::Acquire)
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
        let mut wait = core::pin::pin!(self.input.wait_for_high());
        let mut armed = false;
        poll_fn(|cx| match wait.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(()),
            Poll::Pending => {
                if !armed {
                    // SAFETY: this does not claim ownership of GPIO or modify any field owned
                    // by another pin. GPIO14 is already exclusively owned by this adapter; the
                    // volatile PAC operation only adds its Light-sleep wake-enable bit.
                    unsafe {
                        (*esp32s3::GPIO::ptr())
                            .pin(DIO1_GPIO)
                            .modify(|_, w| w.wakeup_enable().set_bit());
                    }
                    RADIO_WAKE_REGISTRATIONS.fetch_add(1, Ordering::Relaxed);
                    RADIO_WAKE_ARMED.store(true, Ordering::Release);
                    armed = true;
                }
                Poll::Pending
            }
        })
        .await;
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
