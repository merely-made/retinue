//! Endpoint-level resource publish/fetch over the raw interface seam.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, PayloadMode, ReceivedPayload, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::link::{CTX_CACHE_REQUEST, CTX_RESOURCE, CTX_RESOURCE_PRF, CTX_RESOURCE_REQ};
use retinue::lossy::{LossModel, connect};
use retinue::packet::{Packet, PacketType};
use retinue::request::Request;

mod cancel;
mod proofs;
mod transfer;

/// Connect `a` to `b`, dropping `b`'s resource proofs and counting `a`'s cache requests.
/// Only the first proof is dropped, or with `until_asked` every proof until `a` has sent a
/// cache request. Returns the count of proofs dropped and of cache requests.
fn connect_dropping_resource_proofs(
    a: &Endpoint,
    b: &Endpoint,
    until_asked: bool,
) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (mut a_out, a_sink) = a.attach_interface().split();
    let (mut b_out, b_sink) = b.attach_interface().split();
    let dropped = Arc::new(AtomicUsize::new(0));
    let cache_requests = Arc::new(AtomicUsize::new(0));

    let requests = Arc::clone(&cache_requests);
    tokio::spawn(async move {
        while let Some(packet) = a_out.recv().await {
            if packet.context == CTX_CACHE_REQUEST {
                requests.fetch_add(1, Ordering::AcqRel);
            }
            if !b_sink.deliver(packet) {
                break;
            }
        }
    });

    let proofs_dropped = Arc::clone(&dropped);
    let asked = Arc::clone(&cache_requests);
    tokio::spawn(async move {
        while let Some(packet) = b_out.recv().await {
            if packet.context == CTX_RESOURCE_PRF {
                let drop = if until_asked {
                    asked.load(Ordering::Acquire) == 0
                } else {
                    proofs_dropped.load(Ordering::Acquire) == 0
                };
                if drop {
                    proofs_dropped.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
            }
            if !a_sink.deliver(packet) {
                break;
            }
        }
    });

    (dropped, cache_requests)
}

/// Connect `a` to `b`, delivering only the packets `a_to_b` and `b_to_a` pass.
fn connect_filtered(
    a: &Endpoint,
    b: &Endpoint,
    a_to_b: impl Fn(&Packet) -> bool + Send + 'static,
    b_to_a: impl Fn(&Packet) -> bool + Send + 'static,
) {
    let (mut a_out, a_sink) = a.attach_interface().split();
    let (mut b_out, b_sink) = b.attach_interface().split();
    tokio::spawn(async move {
        while let Some(packet) = a_out.recv().await {
            if a_to_b(&packet) && !b_sink.deliver(packet) {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(packet) = b_out.recv().await {
            if b_to_a(&packet) && !a_sink.deliver(packet) {
                break;
            }
        }
    });
}

fn quick(timeout: Duration) -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout,
        retry_interval: Duration::from_millis(50),
        request_window: 4,
    }
}

/// Distinct bytes bz2 cannot shrink below a few parts.
fn incompressible(len: usize) -> Vec<u8> {
    (0..len as u32)
        .flat_map(|n| retinue::hash::full_hash(&n.to_be_bytes()))
        .take(len)
        .collect()
}
