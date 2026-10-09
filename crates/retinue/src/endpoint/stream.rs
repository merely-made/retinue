//! `LinkStream`, the byte stream over a link, and its best-effort driver.

use alloc::vec;
use alloc::vec::Vec;

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::sync::mpsc;

use crate::channel::StreamDecodeError;
use crate::hash::AddressHash;
use crate::link::Link;
use crate::link_liveness::Liveness;
use crate::token::TOKEN_OVERHEAD;

use super::entropy::next_iv;
use super::facts::{LinkDirection, LinkRemoteFact};
use super::interface::InterfaceId;
use super::runtime::{track, track_drainable};
use super::shared::{LinkEntry, LinkKind, Shared};

/// RNS's link MDU (`Link.py` 73, 512-514): the largest plaintext whose CBC-padded token and
/// minimal header fit `mtu` with RNS's one-byte IFAC reserve. 431 at MTU 500. A longer
/// access code is accounted for by the interface's frame limit, which counts it.
const fn link_mdu(mtu: usize) -> usize {
    const IFAC_MIN_SIZE: usize = 1;
    let room = mtu.saturating_sub(IFAC_MIN_SIZE + crate::packet::HEADER_MIN_LEN + TOKEN_OVERHEAD);
    (room / 16 * 16).saturating_sub(1)
}

/// Largest plaintext per link data packet at Reticulum's MTU.
pub(super) const WRITE_CHUNK: usize = link_mdu(crate::packet::MTU);

/// Largest single-packet link plaintext (data, request or response) at the MTU negotiated
/// for this link, so the interface driver never has to reject a packet after `AsyncWrite`
/// already accepted its bytes.
pub(super) fn write_chunk_for_mtu(mtu: u32) -> usize {
    link_mdu(mtu as usize).clamp(1, WRITE_CHUNK)
}

/// In-memory buffer for a stream's inbound side.
pub(super) const DUPLEX_BUF: usize = 64 * 1024;

/// Packets or chunks queued for one link's driver or stream relay. Past this the router
/// drops further link traffic and counts it, instead of buffering without bound: a reliable
/// driver recovers by retransmission, and a best-effort stream fails with an error.
pub(super) const LINK_QUEUE: usize = 256;

/// A bidirectional byte stream over a link.
///
/// Delegates [`AsyncRead`]/[`AsyncWrite`] to an internal duplex; a relay task chunks writes
/// into encrypted link data packets and the endpoint router feeds decrypted inbound data
/// back in. Dropping the stream ends its relay.
pub struct LinkStream {
    pub(super) inner: DuplexStream,
    /// Set by the reliable driver before it drops its duplex half on a terminal failure.
    pub(super) receive_error: Option<Arc<Mutex<Option<StreamFault>>>>,
    /// Set when the link watchdog dropped the link: its end is then a timeout.
    pub(super) lost: Arc<AtomicBool>,
    /// The link id, exposed for diagnostics.
    pub(super) link_id: AddressHash,
    /// The interface this link arrived on (inbound) or was opened over (outbound).
    pub(super) iface: InterfaceId,
}

impl LinkStream {
    /// The id of the link carrying this stream.
    pub fn link_id(&self) -> AddressHash {
        self.link_id
    }

    /// The interface this link arrived on.
    ///
    /// Ingress is a *fact about the session*, so it lives on the stream rather
    /// than only on the accepted-session wrappers: the reliable accept path
    /// surfaces a bare `LinkStream`, and it must report the same ingress as the
    /// best-effort and Resource paths instead of diverging.
    pub fn interface(&self) -> InterfaceId {
        self.iface
    }
}

/// Why a reliable stream ended badly. Its driver records this before dropping its duplex
/// half, so the reader sees an error instead of a clean end of stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamFault {
    /// A received frame could not be decoded.
    Decode(StreamDecodeError),
    /// A sent packet went unproved through every try, so the link was closed (RNS tears a
    /// link down when its channel times out).
    Unacknowledged,
    /// A best-effort link's reader fell so far behind that received bytes were dropped,
    /// so the link was closed rather than leave a hole in the stream.
    Overrun,
}

impl From<StreamDecodeError> for StreamFault {
    fn from(error: StreamDecodeError) -> Self {
        Self::Decode(error)
    }
}

impl AsyncRead for LinkStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) if buf.filled().len() == before => {
                if let Some(error) = self
                    .receive_error
                    .as_ref()
                    .and_then(|state| *state.lock().unwrap())
                {
                    let (kind, message) = match error {
                        StreamFault::Decode(StreamDecodeError::UnsupportedCompression) => (
                            io::ErrorKind::InvalidData,
                            "compressed stream frame unsupported",
                        ),
                        StreamFault::Decode(StreamDecodeError::InvalidCompression) => (
                            io::ErrorKind::InvalidData,
                            "invalid compressed stream frame",
                        ),
                        StreamFault::Decode(StreamDecodeError::DecodedFrameLimitExceeded {
                            ..
                        }) => (
                            io::ErrorKind::InvalidData,
                            "decoded stream frame limit exceeded",
                        ),
                        StreamFault::Unacknowledged => (
                            io::ErrorKind::TimedOut,
                            "reliable link closed: a packet went unproved through every retry",
                        ),
                        StreamFault::Overrun => (
                            io::ErrorKind::Other,
                            "link closed: the reader fell behind and received bytes were lost",
                        ),
                    };
                    Poll::Ready(Err(io::Error::new(kind, message)))
                } else if self.lost.load(Ordering::Acquire) {
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "link timed out: the peer stopped answering",
                    )))
                } else {
                    Poll::Ready(Ok(()))
                }
            }
            result => result,
        }
    }
}

impl AsyncWrite for LinkStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Build a [`LinkStream`] for a live link on `iface`, wiring the inbound feed and the
/// outbound relay, and register the link so the router can route to it.
pub(super) fn register_stream(
    shared: &Arc<Shared>,
    link: Link,
    iface: InterfaceId,
    liveness: Liveness,
    direction: LinkDirection,
    remote: LinkRemoteFact,
) -> Option<LinkStream> {
    let (mine, theirs) = tokio::io::duplex(DUPLEX_BUF);
    let (mut read_half, mut write_half) = tokio::io::split(theirs);
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<Vec<u8>>(LINK_QUEUE);
    let link_id = link.id();
    let write_chunk = write_chunk_for_mtu(link.mtu());
    let lost = Arc::new(AtomicBool::new(false));
    let fault = Arc::new(Mutex::new(None));

    shared.write_diagnostic(|| {
        shared.links.lock().unwrap().insert(
            link_id,
            LinkEntry {
                link: link.clone(),
                kind: LinkKind::BestEffort {
                    inbound: inbound_tx,
                    fault: Arc::clone(&fault),
                },
                iface,
                direction,
                remote,
                liveness,
                lost: Arc::clone(&lost),
            },
        );
        ((), true)
    });

    // Inbound: decrypted data from the router → the stream's read side.
    let inbound_started = track(shared, async move {
        while let Some(bytes) = inbound_rx.recv().await {
            if write_half.write_all(&bytes).await.is_err() {
                break;
            }
        }
        // The inbound channel closed: the link was torn down (a peer link-close, or
        // the endpoint shutting down). Shut the write side explicitly so the reader
        // sees EOF — dropping this half alone would not, since the outbound relay
        // still holds the duplex's read half alive.
        let _ = write_half.shutdown().await;
    });

    // Outbound: the stream's writes → encrypted link data packets, out the link's interface.
    // Drainable: it ends on its own when the stream is dropped, having read the
    // duplex to EOF, so an orderly shutdown can wait for exactly that.
    let out_link = link;
    let iv_shared = Arc::clone(shared);
    let out_lost = Arc::clone(&lost);
    let outbound_started = track_drainable(shared, async move {
        let mut buf = vec![0u8; write_chunk];
        loop {
            let read = read_half.read(&mut buf).await;
            // The watchdog dropped the link and already sent its close: stop, so the
            // writer sees a broken pipe instead of feeding a link nobody hears.
            if out_lost.load(Ordering::Acquire) {
                break;
            }
            match read {
                Ok(0) | Err(_) => {
                    // The stream was shut down or dropped: close the link so the
                    // peer's read side sees EOF. This is what lets a read-to-end
                    // protocol (e.g. gemini) end a response by closing the stream.
                    iv_shared.send_on(iface, out_link.close_packet(&next_iv()));
                    // A link close is final, so nothing more on this link needs routing:
                    // drop its entry (ending the inbound relay) and its inbound slot.
                    iv_shared.remove_link(link_id);
                    break;
                }
                Ok(n) => {
                    let iv = next_iv();
                    iv_shared.send_on(iface, out_link.data_packet(&buf[..n], &iv));
                }
            }
        }
    });

    if !inbound_started || !outbound_started {
        shared.remove_link(link_id);
        return None;
    }

    Some(LinkStream {
        inner: mine,
        receive_error: Some(fault),
        lost,
        link_id,
        iface,
    })
}
