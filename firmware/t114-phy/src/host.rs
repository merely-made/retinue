//! The T114's host link: USB CDC.
//!
//! Native USB, so departure is detectable — a failed write means the host is gone, which
//! `radio-hand`'s dispatch turns into ending the session. Contrast the V4's UART
//! personality, where a write cannot fail and a session therefore never ends.

use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::Driver;
use radio_hand::link::{HostLink, LinkFault};

/// The CDC bulk endpoint's packet size. Chunking to it is this module's business, which is
/// why [`HostLink`] carries no MTU.
const USB_PACKET: usize = 64;

pub struct UsbHost<'d, D: Driver<'d>> {
    class: CdcAcmClass<'d, D>,
}

impl<'d, D: Driver<'d>> UsbHost<'d, D> {
    pub fn new(class: CdcAcmClass<'d, D>) -> Self {
        Self { class }
    }
}

/// Serve a fixed line forever, for a board whose radio never came up.
///
/// The degraded path, and deliberately not a halt: a board that cannot reach its SX1262 is
/// still reachable over USB, still says why, and still takes a DFU package. `sync` keeps
/// answering because it needs no radio and it is how the bench tells this board apart from
/// one that is merely quiet.
pub async fn serve_status_only<'d, D: Driver<'d>>(
    mut class: CdcAcmClass<'d, D>,
    status: &'static [u8],
) -> ! {
    loop {
        // The same terminal gate as `attached()`: a degraded board writing its status into
        // an unread endpoint would block exactly the way a healthy one did.
        class.wait_connection().await;
        while !class.dtr() {
            embassy_time::Timer::after_millis(50).await;
        }
        let mut host = UsbHost::new(class);
        if host.write_all(status).await.is_ok() {
            let mut buffer = [0_u8; USB_PACKET];
            while let Ok(length) = host.read(&mut buffer).await {
                let reply = if &buffer[..length] == b"sync\n" || &buffer[..length] == b"sync\r\n" {
                    b"2b 24b4\r\n".as_slice()
                } else {
                    status
                };
                if host.write_all(reply).await.is_err() {
                    break;
                }
            }
        }
        class = host.class;
    }
}

impl<'d, D: Driver<'d>> HostLink for UsbHost<'d, D> {
    /// Wait for a terminal, not merely for USB.
    ///
    /// `wait_connection()` alone is `wait_enabled()`: it completes as soon as the *device*
    /// is configured, which a plugged-in board always is, host application or none. Gating a
    /// session on it made an unattended board start phantom sessions against nobody — and
    /// because a banner write into an unread endpoint can stall, the board then spent its
    /// life blocked in USB writes instead of selecting on the radio. That was the whole
    /// mechanism of the N4 deafness: `air` showed `beats=0 frames=0` with `txok=1`, meaning
    /// the unattended wait never actually waited.
    ///
    /// A terminal is what asserts DTR, so DTR is the signal. Polled, because embassy-usb
    /// exposes it as a getter; 50 ms is far below human attach latency and costs nothing
    /// against the radio work this select shares.
    async fn attached(&mut self) {
        self.class.wait_connection().await;
        while !self.class.dtr() {
            embassy_time::Timer::after_millis(50).await;
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkFault> {
        // A CDC read yields at most one packet, which is the short read the protocol above
        // already expects.
        let limit = buf.len().min(USB_PACKET);
        self.class
            .read_packet(&mut buf[..limit])
            .await
            .map_err(|_| LinkFault::Detached)
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), LinkFault> {
        for chunk in bytes.chunks(USB_PACKET) {
            self.class
                .write_packet(chunk)
                .await
                .map_err(|_| LinkFault::Detached)?;
        }
        // A USB bulk transfer ends when the host sees a packet SHORTER than the endpoint
        // size. A payload that is an exact multiple of it therefore needs an explicit
        // zero-length packet, or the host keeps waiting for a continuation that never comes
        // and delivers nothing at all.
        //
        // This was silently eating whole replies. The `status` probe answers with a
        // 128-byte banner — exactly two full packets — and returned nothing, while every
        // other probe, none of them a multiple of 64, answered fine. The same trap applies
        // to the data path: an `EVENT_RX` for a 57-byte frame is 7 + 57 = 64 bytes, and was
        // being dropped between the board and the host.
        if !bytes.is_empty() && bytes.len().is_multiple_of(USB_PACKET) {
            self.class
                .write_packet(&[])
                .await
                .map_err(|_| LinkFault::Detached)?;
        }
        Ok(())
    }
}
