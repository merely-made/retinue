//! The V4's host link, for both of its personalities.
//!
//! One implementation covers them because both split into `embedded_io_async` halves: the
//! default USB-serial-JTAG endpoint, and UART0 on the exposed header under
//! `host-uart-low-power`. Which one a binary carries is a compile-time feature, so this
//! stays static generics with no dyn dispatch.
//!
//! Neither reports [`LinkFault::Detached`], and that is the point of the seam rather than a
//! gap in it. The ESP32-S3's USB-serial-JTAG buffers into a peripheral that does not fail a
//! write when a host goes away, and a bare UART has nothing on the other end to notice at
//! all. So `radio-hand`'s shared dispatch, which ends a session only on `Detached`, never
//! ends one here — which is exactly the V4's existing fire-and-forget behaviour, now falling
//! out of the shared loop instead of being written into it.
//!
//! Contrast the T114, whose CDC endpoint does fail a write on departure and therefore does
//! end sessions. Same dispatch, opposite behaviour, decided entirely by the transport.

// Not yet wired into main's loop, deliberately. The V4 is the RF control peer for every
// receipt in this plan, and its loop interleaves `rf-sleep-proof` challenge/response and
// power machinery behind `#[cfg]`s that need their own hardware receipts. This compiles
// as proof that the seam accommodates a second, structurally different transport — split
// rx/tx halves with no detach signal — which is the main thing a seam design can get
// wrong. Wiring the loop is the next session's work, with a counted A/B on real hardware.
#![allow(dead_code)]

use embedded_io_async::{Read, Write};
use radio_hand::link::{HostLink, LinkFault};

pub struct SplitHost<R, W> {
    rx: R,
    tx: W,
}

impl<R: Read, W: Write> SplitHost<R, W> {
    pub fn new(rx: R, tx: W) -> Self {
        Self { rx, tx }
    }
}

impl<R: Read, W: Write> HostLink for SplitHost<R, W> {
    async fn attached(&mut self) {
        // Nothing to wait for: there is no attachment event on either transport, so a
        // session simply begins and the first byte arrives whenever it does.
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkFault> {
        // A read error here is a transport hiccup rather than a departure, so it reports as
        // zero bytes and the session continues. Returning `Detached` would end a session
        // that nothing has actually ended.
        Ok(self.rx.read(buf).await.unwrap_or(0))
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), LinkFault> {
        let _ = self.tx.write_all(bytes).await;
        Ok(())
    }
}
