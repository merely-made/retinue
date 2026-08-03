//! Crash residue and the supervised reboot.
//!
//! Structural decision 4's third boundary: the fault residue goes to supervised reboot.
//! Before this module, the board linked `panic-halt`, so any panic was a dead board until
//! someone reflashed it — the worst failure a field device can have. Now:
//!
//! - **A panic writes its message to noinit RAM and reboots.** The record survives the
//!   reset (the section is never zeroed), so the next boot can say what happened.
//! - **A hard fault does the same**, recording the faulting address.
//! - **A hang is caught by the hardware watchdog**: a starved executor stops petting, the
//!   WDT resets the chip, and the next boot reads the reset reason and counts it as a
//!   crash with `msg=WATCHDOG`.
//! - **A crash loop falls back to the modem channel.** Three consecutive crash boots and
//!   the board stops trusting the persisted personality, boots the one that needs nothing,
//!   and says so. The count clears after a minute of clean running, or by probe.
//!
//! Reboot-as-recovery is honest in this domain: a node blip is ordinary mesh weather, the
//! identity and settings are flash-persisted, and DFU survives any application crash. What
//! this does NOT catch is a stuck `await` that still yields — the executor stays live, the
//! watchdog stays fed, and the hang persists. Catching that class needs per-turn deadlines,
//! which is future work, recorded rather than pretended away.

use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::panic::PanicInfo;

use cortex_m_rt::{ExceptionFrame, exception};

/// "CRSH", so a random cold-boot RAM pattern is vanishingly unlikely to validate.
const MAGIC: u32 = 0x4352_5348;

/// Bytes of crash message kept. Enough for a panic location and a short reason.
pub const MSG_LEN: usize = 96;

/// Consecutive crash boots at which the board stops trusting the persisted channel.
pub const CRASH_LOOP_LIMIT: u32 = 3;

#[repr(C)]
struct CrashRecord {
    magic: u32,
    /// Set by the panic and fault handlers, consumed by the next boot: distinguishes "a
    /// crash just happened" from "an old record rode through a clean soft reset".
    pending: u32,
    /// Consecutive crash boots. Cleared by a clean minute or by probe, never by booting.
    count: u32,
    len: u32,
    msg: [u8; MSG_LEN],
}

// Noinit on purpose: cortex-m-rt zeroes .bss and copies .data, but leaves .uninit alone,
// which is exactly what lets a message written moments before a reset still be there
// moments after.
#[unsafe(link_section = ".uninit.CRASH")]
static mut CRASH: MaybeUninit<CrashRecord> = MaybeUninit::uninit();

/// The record, validated. Cold RAM fails the magic and gets a fresh zeroed record.
///
/// # Safety
///
/// Single-core, and every caller is serialized by construction: the panic and fault
/// handlers are terminal (nothing else runs again), and the boot path runs before any task
/// is spawned.
unsafe fn record() -> &'static mut CrashRecord {
    let slot = &raw mut CRASH;
    let record = unsafe { (*slot).assume_init_mut() };
    if record.magic != MAGIC {
        record.magic = MAGIC;
        record.pending = 0;
        record.count = 0;
        record.len = 0;
        record.msg = [0; MSG_LEN];
    }
    record
}

/// A bounded writer into the record's message buffer. Truncates, never fails.
struct MsgWriter {
    buf: [u8; MSG_LEN],
    at: usize,
}

impl core::fmt::Write for MsgWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &byte in s.as_bytes() {
            if self.at >= MSG_LEN {
                break;
            }
            // Keep it printable, so a probe can echo it raw.
            self.buf[self.at] = if (0x20..0x7f).contains(&byte) {
                byte
            } else {
                b'.'
            };
            self.at += 1;
        }
        Ok(())
    }
}

/// Record a crash and reboot. Terminal.
fn crash_reset(write_msg: impl FnOnce(&mut MsgWriter)) -> ! {
    let mut writer = MsgWriter {
        buf: [0; MSG_LEN],
        at: 0,
    };
    write_msg(&mut writer);
    // SAFETY: terminal context; see `record`.
    let record = unsafe { record() };
    record.pending = 1;
    record.count = record.count.saturating_add(1);
    record.len = writer.at as u32;
    record.msg = writer.buf;
    cortex_m::peripheral::SCB::sys_reset();
}

/// The panic handler: residue, then reboot. Replaces `panic-halt`, whose halt-forever was
/// the one behavior a field device must never have.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crash_reset(|w| {
        let _ = write!(w, "{info}");
    })
}

/// Hard faults get the same treatment, with the faulting address for the record.
#[exception]
unsafe fn HardFault(frame: &ExceptionFrame) -> ! {
    crash_reset(|w| {
        let _ = write!(w, "HARDFAULT pc={:08x} lr={:08x}", frame.pc(), frame.lr());
    })
}

/// What the boot found in the residue.
pub struct BootCrash {
    /// Consecutive crash boots, this one included if it is one.
    pub count: u32,
    /// Whether the persisted channel should be distrusted this boot.
    pub fallback: bool,
    /// Why the chip reset, per RESETREAS.
    pub reset: &'static str,
}

/// Read the residue and the reset reason. Call once, before tasks spawn.
pub fn on_boot() -> BootCrash {
    let resetreas = embassy_nrf::pac::POWER.resetreas().read();
    let reset = if resetreas.dog() {
        "watchdog"
    } else if resetreas.sreq() {
        "soft"
    } else if resetreas.resetpin() {
        "pin"
    } else if resetreas.lockup() {
        "lockup"
    } else {
        "power"
    };
    // Write-one-to-clear, so the next boot's reasons are its own.
    embassy_nrf::pac::POWER.resetreas().write_value(resetreas);

    // SAFETY: boot path, before any task exists; see `record`.
    let record = unsafe { record() };

    if record.pending != 0 {
        // A panic or hard fault wrote this moments before the reset; the count is already
        // bumped. Consume the flag so a later clean soft reset is not miscounted.
        record.pending = 0;
    } else if resetreas.dog() || resetreas.lockup() {
        // The watchdog or a lockup reset us without a handler running: count it here and
        // name it, so a hang loop trips the fallback exactly like a panic loop.
        record.count = record.count.saturating_add(1);
        let label: &[u8] = if resetreas.dog() {
            b"WATCHDOG"
        } else {
            b"LOCKUP"
        };
        record.msg[..label.len()].copy_from_slice(label);
        record.len = label.len() as u32;
    }

    BootCrash {
        count: record.count,
        fallback: record.count >= CRASH_LOOP_LIMIT,
        reset,
    }
}

/// The residue, for the `crash` probe: (consecutive crash boots, last message).
pub fn residue() -> (u32, &'static [u8]) {
    // SAFETY: single-core; probes run from the one executor thread, and the handlers that
    // also touch the record are terminal.
    let record = unsafe { record() };
    let len = (record.len as usize).min(MSG_LEN);
    (record.count, &record.msg[..len])
}

/// Clear the crash count, keeping the last message for post-mortem reading.
///
/// Called by the clean-run timer (a minute of living proves the boot) and by probe.
pub fn clear_count() {
    // SAFETY: as `residue`.
    let record = unsafe { record() };
    record.count = 0;
}

/// Forget everything, by probe.
pub fn clear_all() {
    // SAFETY: as `residue`.
    let record = unsafe { record() };
    record.count = 0;
    record.len = 0;
    record.msg = [0; MSG_LEN];
}

/// Feed the hardware watchdog forever.
///
/// The watchdog catches what the panic handler cannot: busy-loops and lockups that starve
/// the executor. While the executor breathes, this task pets; when it stops breathing, the
/// chip resets and the next boot says `reset=watchdog`.
#[embassy_executor::task]
pub async fn watchdog_task(mut handle: embassy_nrf::wdt::WatchdogHandle) {
    loop {
        handle.pet();
        embassy_time::Timer::after_secs(2).await;
    }
}

/// After a clean minute, the crash count no longer describes this boot.
#[embassy_executor::task]
pub async fn clean_run_task() {
    embassy_time::Timer::after_secs(60).await;
    clear_count();
}
