//! Carrier pumps: one HDLC-framed byte stream per interface, both directions in one task.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::iface::hdlc::{Deframer, frame};
use crate::packet::Packet;

use super::interface::InterfaceId;
use super::queue::OutboundPackets;
use super::shared::Shared;

/// A write that makes no progress for this long ends the pump, as Backbone's egress dead time
/// does (`BackboneInterface.py` 63-69, 476-499): a peer that stops reading must not keep its
/// interface, and the routes through it, alive.
pub(super) const DEAD_TIME: Duration = Duration::from_secs(12);

/// Why a pump ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PumpEnd {
    /// The peer closed the stream.
    Eof,
    /// A write made no progress for [`DEAD_TIME`].
    WriteStall,
    /// The stream failed.
    Error,
    /// The endpoint closed the interface or stopped routing.
    Closed,
}

/// Carry interface `id` over one byte stream until either direction ends, which ends both: a
/// writer that fails must not leave the reader holding a half-dead interface.
///
/// Inbound frames are IFAC-opened and decoded; a full router queue is awaited rather than
/// dropped, so TCP flow control slows a flooding peer. A queued packet that cannot be encoded
/// is dropped and counted, and the pump carries on.
pub(super) async fn run<R, W>(
    shared: &Shared,
    id: InterfaceId,
    out: &mut OutboundPackets,
    mut reader: R,
    mut writer: W,
) -> PumpEnd
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let ifac = out.ifac.clone();
    let write = async {
        while let Some(pkt) = out.recv().await {
            let Ok(wire) = out.encode(&pkt) else {
                shared.with_iface(id, |iface| iface.unsendable.fetch_add(1, Ordering::Relaxed));
                continue;
            };
            let sent = async {
                writer.write_all(&frame(&wire)).await?;
                writer.flush().await
            };
            match tokio::time::timeout(DEAD_TIME, sent).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return PumpEnd::Error,
                Err(_) => return PumpEnd::WriteStall,
            }
        }
        PumpEnd::Closed
    };
    let read = async {
        let mut deframer = Deframer::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = match reader.read(&mut buf).await {
                Ok(0) => return PumpEnd::Eof,
                Ok(n) => n,
                Err(_) => return PumpEnd::Error,
            };
            for raw in deframer.push(&buf[..n]) {
                let logical = match &ifac {
                    Some(ifac) => match ifac.open(&raw) {
                        Ok(logical) => logical,
                        Err(_) => continue,
                    },
                    None => raw,
                };
                if let Ok(pkt) = Packet::decode(&logical)
                    && shared.router_tx.send((id, pkt)).await.is_err()
                {
                    return PumpEnd::Closed;
                }
            }
        }
    };
    tokio::select! {
        end = write => end,
        end = read => end,
    }
}

/// Carry one TCP connection: online while it lasts, then offline with the backlog discarded,
/// since RNS sends nothing on an interface that is down (`Transport.py` 1449).
pub(super) async fn carry_tcp(
    shared: &Shared,
    id: InterfaceId,
    online: &AtomicBool,
    out: &mut OutboundPackets,
    stream: TcpStream,
) -> PumpEnd {
    out.discard();
    online.store(true, Ordering::Release);
    let (reader, writer) = stream.into_split();
    let end = run(shared, id, out, reader, writer).await;
    online.store(false, Ordering::Release);
    out.discard();
    end
}
