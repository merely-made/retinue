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
#[cfg(feature = "host-usb")]
const USB_PACKET_BYTES: usize = 64;

/// Write one USB Serial/JTAG record with an unambiguous final packet.
///
/// esp-hal completes every 64-byte chunk immediately. Splitting the final byte of an exact
/// packet multiple therefore gives the host a terminal short packet without changing the byte
/// stream. UART does not use this helper because it has no USB packet boundary.
#[cfg(feature = "host-usb")]
async fn write_usb_record<W: Write>(tx: &mut W, bytes: &[u8]) -> Result<(), LinkFault> {
    if !bytes.is_empty() && bytes.len() % USB_PACKET_BYTES == 0 {
        let (whole_packets, final_byte) = bytes.split_at(bytes.len() - 1);
        tx.write_all(whole_packets)
            .await
            .map_err(|_| LinkFault::Detached)?;
        tx.write_all(final_byte)
            .await
            .map_err(|_| LinkFault::Detached)?;
    } else {
        tx.write_all(bytes).await.map_err(|_| LinkFault::Detached)?;
    }
    tx.flush().await.map_err(|_| LinkFault::Detached)
}

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
        // hardware round trip to find. Exact USB packet multiples also need a terminal short
        // packet, otherwise the host has no record boundary to deliver.
        #[cfg(feature = "host-usb")]
        let _ = write_usb_record(&mut self.tx, bytes).await;
        #[cfg(feature = "host-uart-low-power")]
        {
            let _ = self.tx.write_all(bytes).await;
            let _ = self.tx.flush().await;
        }
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
                write_usb_record(&mut self.tx, bytes).await
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

#[cfg(all(test, feature = "host-usb"))]
mod tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embedded_io_async::{Error, ErrorKind, ErrorType};

    #[derive(Debug, Clone, Copy)]
    enum MockError {
        Write,
        Flush,
    }

    impl core::fmt::Display for MockError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "mock I/O failure")
        }
    }

    impl core::error::Error for MockError {}

    impl Error for MockError {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    struct MockReader;

    impl ErrorType for MockReader {
        type Error = MockError;
    }

    impl Read for MockReader {
        async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, Self::Error> {
            Ok(0)
        }
    }

    struct MockWriter {
        writes: [WriteRecord; 2],
        write_count: usize,
        flushes: usize,
        fail_write: Option<usize>,
        fail_flush: bool,
    }

    #[derive(Clone, Copy)]
    struct WriteRecord {
        bytes: [u8; USB_PACKET_BYTES * 2],
        len: usize,
    }

    impl WriteRecord {
        const EMPTY: Self = Self {
            bytes: [0; USB_PACKET_BYTES * 2],
            len: 0,
        };
    }

    impl MockWriter {
        fn healthy() -> Self {
            Self {
                writes: [WriteRecord::EMPTY; 2],
                write_count: 0,
                flushes: 0,
                fail_write: None,
                fail_flush: false,
            }
        }
    }

    impl ErrorType for MockWriter {
        type Error = MockError;
    }

    impl Write for MockWriter {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            let write_number = self.write_count + 1;
            if self.fail_write == Some(write_number) {
                return Err(MockError::Write);
            }
            let record = &mut self.writes[self.write_count];
            record.bytes[..bytes.len()].copy_from_slice(bytes);
            record.len = bytes.len();
            self.write_count += 1;
            Ok(bytes.len())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            self.flushes += 1;
            if self.fail_flush {
                Err(MockError::Flush)
            } else {
                Ok(())
            }
        }
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test mock must not block"),
        }
    }

    #[test]
    fn usb_exact_packet_multiples_end_with_a_short_write() {
        for length in [64, 128] {
            let bytes = [0xA5; USB_PACKET_BYTES * 2];
            let mut host = SplitHost::new(MockReader, MockWriter::healthy());

            assert_eq!(block_on(host.write_all(&bytes[..length])), Ok(()));
            assert_eq!(host.tx.write_count, 2);
            assert_eq!(host.tx.writes[0].len, length - 1);
            assert_eq!(host.tx.writes[1].len, 1);
            assert_eq!(host.tx.writes[0].bytes[..length - 1], bytes[..length - 1]);
            assert_eq!(host.tx.writes[1].bytes[..1], bytes[length - 1..length]);
            assert_eq!(host.tx.flushes, 1);
        }
    }

    #[test]
    fn usb_non_multiple_is_one_write() {
        let bytes = [0x5A; USB_PACKET_BYTES - 1];
        let mut host = SplitHost::new(MockReader, MockWriter::healthy());

        assert_eq!(block_on(host.write_all(&bytes)), Ok(()));
        assert_eq!(host.tx.write_count, 1);
        assert_eq!(host.tx.writes[0].len, bytes.len());
        assert_eq!(host.tx.writes[0].bytes[..bytes.len()], bytes);
        assert_eq!(host.tx.flushes, 1);
    }

    #[test]
    fn diagnostic_write_fault_latches_the_host_session() {
        let mut writer = MockWriter::healthy();
        writer.fail_write = Some(2);
        let mut host = SplitHost::new(MockReader, writer);

        assert_eq!(
            block_on(host.write_diagnostic(&[0xD4; USB_PACKET_BYTES])),
            Err(LinkFault::Detached)
        );
        assert!(host.diagnostic_retired);
        assert_eq!(block_on(host.write_all(&[1])), Err(LinkFault::Detached));
        assert_eq!(host.tx.write_count, 1);
        assert_eq!(host.tx.writes[0].len, USB_PACKET_BYTES - 1);
        assert_eq!(
            host.tx.writes[0].bytes[..USB_PACKET_BYTES - 1],
            [0xD4; USB_PACKET_BYTES - 1]
        );
    }

    #[test]
    fn diagnostic_flush_fault_latches_the_host_session() {
        let mut writer = MockWriter::healthy();
        writer.fail_flush = true;
        let mut host = SplitHost::new(MockReader, writer);

        assert_eq!(
            block_on(host.write_diagnostic(&[0xD5; USB_PACKET_BYTES - 1])),
            Err(LinkFault::Detached)
        );
        assert!(host.diagnostic_retired);
        assert_eq!(host.tx.write_count, 1);
        assert_eq!(host.tx.flushes, 1);
    }
}
