//! The byte-for-byte TCP relay between RNS and the endpoint, with its packet tally.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use retinue::iface::hdlc::{Deframer, frame};
use retinue::link::{CTX_CACHE_REQUEST, CTX_RESOURCE_PRF};
use retinue::packet::{Packet, PacketType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub(super) type Tally = Arc<Mutex<BTreeMap<(&'static str, &'static str, u8), u64>>>;

fn type_name(packet_type: PacketType) -> &'static str {
    match packet_type {
        PacketType::Data => "Data",
        PacketType::Announce => "Announce",
        PacketType::LinkRequest => "LinkRequest",
        PacketType::Proof => "Proof",
    }
}

/// How a relay direction treats resource proofs and cache requests, for the proof-cache
/// gate. Both directions share the flag that says a cache request has been heard.
#[derive(Clone)]
enum ProofFilter {
    /// Copy everything verbatim.
    None,
    /// Re-frame packet by packet, leaving out (and tallying as `Dropped`) every resource
    /// proof until the flag is set.
    DropUntilAsked(Arc<AtomicBool>),
    /// Copy verbatim, setting the flag on the first cache request.
    MarkAsked(Arc<AtomicBool>),
}

/// Copy one direction verbatim, tallying a deframed copy of every packet, and filtering
/// resource proofs as `filter` says.
async fn relay(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    direction: &'static str,
    tally: Tally,
    filter: ProofFilter,
) {
    let mut deframer = Deframer::new();
    let mut buf = vec![0_u8; 8192];
    let filtering = matches!(filter, ProofFilter::DropUntilAsked(_));
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        // A cache request is marked before it is passed on, so the answer it draws can
        // never reach the other direction ahead of the mark.
        let marking = matches!(filter, ProofFilter::MarkAsked(_));
        if !filtering && !marking && to.write_all(&buf[..n]).await.is_err() {
            break;
        }
        for raw in deframer.push(&buf[..n]) {
            let Ok(packet) = Packet::decode(&raw) else {
                continue;
            };
            let mut kind = type_name(packet.packet_type);
            if let ProofFilter::MarkAsked(asked) = &filter
                && packet.context == CTX_CACHE_REQUEST
            {
                asked.store(true, Ordering::Release);
            }
            if let ProofFilter::DropUntilAsked(asked) = &filter
                && !asked.load(Ordering::Acquire)
                && packet.packet_type == PacketType::Proof
                && packet.context == CTX_RESOURCE_PRF
            {
                kind = "Dropped";
            } else if filtering && to.write_all(&frame(&raw)).await.is_err() {
                return;
            }
            *tally
                .lock()
                .unwrap()
                .entry((direction, kind, packet.context))
                .or_default() += 1;
        }
        if marking && to.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
    let _ = to.shutdown().await;
}

/// Accept the RNS connection on `listener` and relay it to the endpoint at `inner`.
pub(super) async fn tap(
    listener: TcpListener,
    inner: SocketAddr,
    tally: Tally,
    drop_proofs: bool,
) -> std::io::Result<()> {
    let (outer, _) = listener.accept().await?;
    let endpoint_side = TcpStream::connect(inner).await?;
    let (outer_read, outer_write) = outer.into_split();
    let (inner_read, inner_write) = endpoint_side.into_split();
    let (to_retinue, to_rns) = if drop_proofs {
        let asked = Arc::new(AtomicBool::new(false));
        (
            ProofFilter::MarkAsked(Arc::clone(&asked)),
            ProofFilter::DropUntilAsked(asked),
        )
    } else {
        (ProofFilter::None, ProofFilter::None)
    };
    tokio::spawn(relay(
        outer_read,
        inner_write,
        "to_retinue",
        tally.clone(),
        to_retinue,
    ));
    tokio::spawn(relay(inner_read, outer_write, "to_rns", tally, to_rns));
    Ok(())
}

pub(super) fn print_tally(tally: &Tally) {
    for ((direction, packet_type, context), count) in tally.lock().unwrap().iter() {
        println!("TAP {direction} {packet_type} ctx=0x{context:02x} {count}");
    }
}
