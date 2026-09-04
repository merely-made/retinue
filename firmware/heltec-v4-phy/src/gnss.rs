//! Heltec V4 GNSS socket: PD3's board half. Owns UART1 and the three control pins,
//! feeds bytes to `radio_hand::gnss::NmeaParser`, and publishes the latest
//! [`GnssState`] for whoever assembles `LocalStatus`.
//!
//! Pin map, from Heltec's V4 GNSS interface (SH1.25 8-pin) and factory sketch:
//! `Serial1.begin(9600, SERIAL_8N1, 39, 38)`, whose final pins are RX then TX.
//! Module TX therefore enters ESP RX on GPIO39. This read-only driver leaves module
//! RX / ESP TX GPIO38 unowned, power enable GPIO34 active low, reset GPIO42 held high
//! to run, and standby GPIO40 held high to keep the module awake. GPIO34 controls the
//! GNSS rail; the OLED's Vext GPIO36 is unrelated.
//!
//! The UART runs at the L76K's 9600 factory default. Nothing is sent to the module:
//! the board reads what it emits by default, `RMC` and `GGA` among others, and the
//! parser drops the rest.

use core::cell::Cell;

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::Instant;
use esp_hal::Async;
use esp_hal::gpio::Output;
use esp_hal::uart::UartRx;
use radio_face::GnssState;
use radio_hand::gnss::NmeaParser;

/// The L76K's factory default. Nothing here reconfigures the module, so this is the
/// only rate it will ever be read at.
pub const BAUD: u32 = 9_600;

/// Latest state the parser produced. `Absent` until the first accepted sentence.
static LATEST: Mutex<CriticalSectionRawMutex, Cell<GnssState>> =
    Mutex::new(Cell::new(GnssState::Absent));

/// The most recent GNSS state. Cheap; safe from any task or interrupt context.
pub fn latest() -> GnssState {
    LATEST.lock(|cell| cell.get())
}

/// Parser and UART-error counters. `bytes` counts successful reads only, so a zero
/// byte count is inconclusive when `errors` is nonzero. All counters saturate.
static COUNTERS: Mutex<CriticalSectionRawMutex, Cell<(u32, u32, u32, u32)>> =
    Mutex::new(Cell::new((0, 0, 0, 0)));

pub fn counters() -> (u32, u32, u32, u32) {
    COUNTERS.lock(|cell| cell.get())
}

/// Control pins the task holds for its lifetime so they are never dropped back to
/// their reset state while the module is meant to be running. They are set once at
/// construction and never read again; holding them is their whole job.
#[allow(dead_code)]
pub struct ControlPins {
    pub enable: Output<'static>,
    pub reset: Output<'static>,
    pub standby: Output<'static>,
}

/// Reads NMEA from the module forever. The three outputs are moved in and kept.
#[embassy_executor::task]
pub async fn gnss_task(mut rx: UartRx<'static, Async>, _pins: ControlPins) {
    let mut parser = NmeaParser::new();
    let mut buf = [0u8; 64];
    loop {
        let read = match rx.read_async(&mut buf).await {
            Ok(n) => n,
            Err(_) => {
                // `RxError` is non-exhaustive. A compact total remains useful for every
                // present and future kind, including the framing errors a wrong baud causes.
                COUNTERS.lock(|cell| {
                    let (accepted, dropped, bytes, errors) = cell.get();
                    cell.set((accepted, dropped, bytes, errors.saturating_add(1)));
                });
                continue;
            }
        };
        let uptime = Instant::now().as_secs() as u32;
        for &byte in &buf[..read] {
            if let Some(state) = parser.push(byte, uptime) {
                LATEST.lock(|cell| cell.set(state));
            }
        }
        COUNTERS.lock(|cell| {
            let (_, _, bytes, errors) = cell.get();
            cell.set((
                parser.accepted(),
                parser.dropped(),
                bytes.saturating_add(read as u32),
                errors,
            ));
        });
    }
}
