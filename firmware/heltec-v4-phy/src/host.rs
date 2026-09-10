//! The V4's host link, for both of its personalities.
//!
//! One implementation covers them because both split into `embedded_io_async` halves: the
//! default USB-serial-JTAG endpoint, and UART0 on the exposed header under
//! `host-uart-low-power`. Which one a binary carries is a compile-time feature, so this
//! stays static generics with no dyn dispatch.
//!
//! Neither personality exposes physical attachment state. Ordinary writes therefore retain
//! the V4's fire-and-forget behavior. On the active-radio USB personality, diagnostic writes
//! have a short software deadline and report [`LinkFault::Detached`] when the write or flush
//! fails or times out. Such a fault latches all host I/O closed until an external board reset,
//! so no later traffic can extend a partial response while the independent radio branch
//! remains available. The UART low-power personality continues to decline diagnostic writes;
//! its carrier behavior remains part of the separate sleep-edge receipt.
//!
//! Contrast the T114, whose CDC endpoint does fail a write on departure and therefore does
//! end sessions. Same dispatch, opposite behaviour, decided entirely by the transport.

// The active-radio USB image wires this transport into main's shared command dispatch. The
// same generic adapter also compiles for the UART low-power personality, whose timing and
// sleep-edge behavior require their own physical receipt.
#![allow(dead_code)]

use embedded_io_async::{Read, Write};
use radio_hand::link::{HostLink, LinkFault};

#[cfg(feature = "host-usb")]
const DIAGNOSTIC_WRITE_DEADLINE: embassy_time::Duration = embassy_time::Duration::from_millis(250);

pub struct SplitHost<R, W> {
    rx: R,
    tx: W,
    diagnostic_retired: bool,
}

impl<R: Read, W: Write> SplitHost<R, W> {
    pub fn new(rx: R, tx: W) -> Self {
        Self {
            rx,
            tx,
            diagnostic_retired: false,
        }
    }
}

impl<R: Read, W: Write> HostLink for SplitHost<R, W> {
    async fn attached(&mut self) {
        // Nothing to wait for: there is no attachment event on either transport, so a
        // session simply begins and the first byte arrives whenever it does.
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkFault> {
        if self.diagnostic_retired {
            // Keep this select branch dormant while the independent radio branch continues.
            // Only an explicit external board reset establishes a fresh host session.
            return core::future::pending().await;
        }
        // A read error here is a transport hiccup rather than a departure, so it reports as
        // zero bytes and the session continues. Returning `Detached` would end a session
        // that nothing has actually ended.
        Ok(self.rx.read(buf).await.unwrap_or(0))
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), LinkFault> {
        if self.diagnostic_retired {
            return Err(LinkFault::Detached);
        }
        // The flush is load-bearing, not hygiene. USB Serial/JTAG holds written bytes in the
        // peripheral until flushed, so an unflushed reply simply never reaches the host: the
        // board looks alive and answers nothing. Dropping it while building this seam cost a
        // hardware round trip to find.
        let _ = self.tx.write_all(bytes).await;
        let _ = self.tx.flush().await;
        Ok(())
    }

    async fn write_diagnostic(&mut self, bytes: &[u8]) -> Result<(), LinkFault> {
        #[cfg(feature = "host-uart-low-power")]
        {
            let _ = bytes;
            return Err(LinkFault::Detached);
        }
        #[cfg(feature = "host-usb")]
        {
            if self.diagnostic_retired {
                return Err(LinkFault::Detached);
            }
            // Observation replies are best-effort telemetry, so they must not hold the radio
            // owner indefinitely when USB is connected electrically but the host has stopped
            // draining it. After any timeout or write fault, latch this diagnostic carrier closed
            // until an external board reset: no later reply can append to a possibly partial
            // response, while the independent RF branch remains available. Latch before the
            // first await so cancellation is fail-closed too; only full success clears it.
            self.diagnostic_retired = true;
            let result = embassy_time::with_timeout(DIAGNOSTIC_WRITE_DEADLINE, async {
                self.tx
                    .write_all(bytes)
                    .await
                    .map_err(|_| LinkFault::Detached)?;
                self.tx.flush().await.map_err(|_| LinkFault::Detached)
            })
            .await
            .unwrap_or(Err(LinkFault::Detached));
            if result.is_ok() {
                self.diagnostic_retired = false;
            }
            result
        }
    }
}
