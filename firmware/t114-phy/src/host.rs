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

impl<'d, D: Driver<'d>> HostLink for UsbHost<'d, D> {
    async fn attached(&mut self) {
        self.class.wait_connection().await;
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
        Ok(())
    }
}
