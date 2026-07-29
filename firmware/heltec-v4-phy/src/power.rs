//! Guarded Light-sleep for the low-power host personality.
//!
//! The radio can listen while the CPU sleeps: the SX1262 stays in continuous receive and
//! raises DIO1 on a packet, and the host can rouse the chip over UART0. What the CPU must not
//! do is sleep in the middle of something it cannot resume — a half-parsed command, an SPI
//! transaction, a transmission, a partly-written host event.
//!
//! So sleeping is *gated* rather than automatic. The idle hook sleeps only when the gate is
//! open, and the command loop closes it around every critical section. The gate is a counter,
//! not a flag, so nested holds compose: an inner hold cannot re-open the gate an outer one is
//! still relying on.
//!
//! # What is deliberately not attempted
//!
//! Sleeping across an Embassy timer deadline. The time source this runtime uses does not
//! advance in Light-sleep, so a sleep that spans a deadline silently corrupts the schedule.
//! Timer-aware sleep needs clock compensation, which is its own design. Until then the gate
//! is closed whenever a timer is pending.

//! # Two builds, one call site
//!
//! Everything here exists in both personalities. Under `host-usb` the gate is a no-op, so the
//! shared command loop takes the same holds either way and cannot drift between builds.

#[cfg(not(feature = "host-uart-low-power"))]
pub use stub::*;

/// The USB personality never sleeps, so the gate costs nothing and does nothing.
#[cfg(not(feature = "host-uart-low-power"))]
mod stub {
    /// A hold that guards nothing, because this build never sleeps.
    pub struct Awake;

    impl Awake {
        pub fn new() -> Self {
            Self
        }
    }

    /// Always zero: this build has no idle hook and never sleeps. Answering rather than
    /// staying silent lets the bench distinguish "not sleeping" from "wrong firmware".
    pub fn counters() -> (u32, u32) {
        (0, 0)
    }

    pub fn last_sleep_us() -> u32 {
        0
    }
}

#[cfg(feature = "host-uart-low-power")]
pub use low_power::*;

#[cfg(feature = "host-uart-low-power")]
mod low_power {
    use core::cell::RefCell;
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    #[cfg(feature = "rf-sleep-proof")]
    use core::time::Duration;

    use critical_section::Mutex;
    use esp_hal::rtc_cntl::Rtc;
    #[cfg(feature = "rf-sleep-proof")]
    use esp_hal::rtc_cntl::sleep::TimerWakeupSource;
    use esp_hal::rtc_cntl::sleep::{
        GpioWakeupSource, RtcSleepConfig, Uart0WakeupSource, WakeSource,
    };

    /// Rising edges on UART0 RXD needed to wake the chip.
    ///
    /// The wake itself consumes the edges that triggered it, so the bytes carrying them are lost.
    /// That is why the host sends a run of [`selvage::WAKE_BYTE`] before a command rather than the
    /// command itself: the preamble is what gets eaten.
    const UART_WAKE_THRESHOLD: u16 = 3;

    /// Open holds on the gate. Sleeping is permitted only at zero.
    ///
    /// Starts held: nothing may sleep until [`arm`] says the radio is receiving and the command
    /// loop is at a frame boundary.
    static HOLDS: AtomicUsize = AtomicUsize::new(1);

    /// Times the idle hook actually entered Light-sleep.
    static SLEEP_ENTRIES: AtomicU32 = AtomicU32::new(0);
    /// RTC time spent in the most recent task-integrated proof sleep.
    static LAST_SLEEP_US: AtomicU32 = AtomicU32::new(0);
    /// Times the idle hook ran but the gate was closed.
    static SLEEP_BLOCKED: AtomicU32 = AtomicU32::new(0);

    /// The RTC peripheral, parked for the idle hook.
    ///
    /// A critical section rather than a `static mut`: the hook runs from the scheduler and the
    /// setup path writes this once, and the two must not race on a dual-core part.
    static RTC: Mutex<RefCell<Option<Rtc<'static>>>> = Mutex::new(RefCell::new(None));

    /// ESP-HAL 1.1.1 predates esp-rs/esp-hal#5777. Its ESP32-S3 default leaves the
    /// XTAL-down regulator/bias state incomplete, which resets the chip on Light-sleep.
    fn sleep_light(rtc: &mut Rtc<'static>, wake_sources: &[&dyn WakeSource]) {
        let mut config = RtcSleepConfig::default();
        config.set_rtc_regulator_fpu(true);
        config.set_bias_sleep_monitor(true);
        config.set_pd_cur_monitor(true);
        config.set_bias_sleep_slp(true);
        config.set_pd_cur_slp(true);
        rtc.sleep(&config, wake_sources);
    }

    /// Hand the RTC to the idle hook and open the gate for the first time.
    ///
    /// Call once, after the radio is armed for receive and before the command loop starts.
    pub fn arm(rtc: Rtc<'static>) {
        critical_section::with(|cs| {
            RTC.borrow(cs).replace(Some(rtc));
        });
        release();
    }

    fn hold() {
        HOLDS.fetch_add(1, Ordering::Acquire);
    }

    fn release() {
        HOLDS.fetch_sub(1, Ordering::Release);
    }

    /// Whether the idle hook may sleep right now.
    pub fn may_sleep() -> bool {
        HOLDS.load(Ordering::Acquire) == 0
    }

    /// Sleep entries and blocked-idle counts, for the power receipt.
    pub fn counters() -> (u32, u32) {
        (
            SLEEP_ENTRIES.load(Ordering::Relaxed),
            SLEEP_BLOCKED.load(Ordering::Relaxed),
        )
    }

    pub fn last_sleep_us() -> u32 {
        LAST_SLEEP_US.load(Ordering::Relaxed)
    }

    /// Enter one proof sleep while the radio receive future remains pinned in its task.
    ///
    /// The five-second timer is a safety ceiling and a diagnostic: a much shorter elapsed
    /// interval means GPIO activity woke the CPU before the timer did.
    #[cfg(feature = "rf-sleep-proof")]
    pub fn sleep_once(rtc: &mut Rtc<'static>) -> u32 {
        let gpio = GpioWakeupSource::new();
        // Deliberately longer than the witness's RF challenge interval. A shorter reported
        // sleep therefore identifies GPIO wake rather than timer fallback.
        let timer = TimerWakeupSource::new(Duration::from_secs(5));
        let started = rtc.current_time_us();
        // A scheduler interrupt may not context-switch through the ROM's blocking sleep
        // transition. Hold the current task in place until the clocks and CPU state are restored.
        critical_section::with(|_| sleep_light(rtc, &[&gpio, &timer]));
        let elapsed = rtc
            .current_time_us()
            .saturating_sub(started)
            .min(u64::from(u32::MAX)) as u32;
        if elapsed >= 1_000 {
            LAST_SLEEP_US.store(elapsed, Ordering::Relaxed);
            SLEEP_ENTRIES.fetch_add(1, Ordering::Relaxed);
        } else {
            // A pending interrupt can reject Light-sleep before clocks are dropped.
            SLEEP_BLOCKED.fetch_add(1, Ordering::Relaxed);
        }
        elapsed
    }

    /// Holds the gate closed for as long as it lives.
    ///
    /// Taking one of these around a critical section is the whole discipline: it cannot be
    /// forgotten on an early return or an error path, because the drop does the releasing.
    pub struct Awake;

    impl Awake {
        /// Keep the CPU awake until this is dropped.
        pub fn new() -> Self {
            hold();
            Self
        }
    }

    impl Drop for Awake {
        fn drop(&mut self) {
            release();
        }
    }

    /// The idle hook: sleep when the gate allows, otherwise wait for an interrupt.
    ///
    /// Never returns — this replaces the scheduler's idle loop.
    pub extern "C" fn idle() -> ! {
        loop {
            if !may_sleep() {
                SLEEP_BLOCKED.fetch_add(1, Ordering::Relaxed);
                // Something is mid-flight. Wait for an interrupt without dropping the clocks.
                esp_hal::interrupt::wait_for_interrupt();
                continue;
            }

            // Wake on radio activity (DIO1 is a GPIO interrupt) or on the host talking to us.
            let gpio = GpioWakeupSource::new();
            let uart = Uart0WakeupSource::new(UART_WAKE_THRESHOLD);
            #[cfg(feature = "rf-sleep-proof")]
            let timer = TimerWakeupSource::new(Duration::from_millis(500));
            let slept = critical_section::with(|cs| {
                let mut slot = RTC.borrow(cs).borrow_mut();
                match slot.as_mut() {
                    Some(rtc) => {
                        #[cfg(not(feature = "rf-sleep-proof"))]
                        sleep_light(rtc, &[&gpio, &uart]);
                        // Proof-only recovery source. It distinguishes actual Light-sleep plus
                        // retained radio receive from the stricter claim that DIO1 itself woke
                        // the CPU. Production remains GPIO + UART only.
                        #[cfg(feature = "rf-sleep-proof")]
                        sleep_light(rtc, &[&gpio, &uart, &timer]);
                        true
                    }
                    // Not armed yet: nothing to sleep with.
                    None => false,
                }
            });
            if slept {
                SLEEP_ENTRIES.fetch_add(1, Ordering::Relaxed);
            } else {
                esp_hal::interrupt::wait_for_interrupt();
            }
        }
    }
}
