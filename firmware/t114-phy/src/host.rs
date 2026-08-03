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
        class.wait_connection().await;
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
