//! The Retinue half of the library-path live oracle gates.
//!
//! Every mode drives the public `Endpoint` API, never the resource or channel state
//! machines directly, so a passing gate is evidence about what applications get.
//! The Python drivers in `oracle/` play stock RNS on the other end:
//!
//! - `resource-recv LEN SEED` (`interop_library_resource_recv.py`): RNS sends a Resource;
//!   `Endpoint::accept_resource` and `ResourceSession::receive` take it.
//! - `resource-send LEN SEED` (`interop_library_resource_send.py`): a `ResourceSession`
//!   opened with `Endpoint::open_resource` publishes to an RNS receiver.
//! - `stream-respond LEN SEED` (`interop_reliable_responder_send.py`): RNS opens a reliable
//!   link and reads what `Endpoint::accept_reliable`'s stream writes.
//! - `stream-open LEN SEED` (`interop_reliable_initiator_proofs.py`): an
//!   `Endpoint::open_reliable` stream reads what an RNS responder's Channel sends.
//! - `liveness SECS SEED` (`interop_link_liveness.py`): one resource link each way with
//!   RNS, held idle for `SECS` seconds, then a request of exactly the 431-byte link MDU in
//!   each direction.
//! - `resource-recv-drop-proof LEN SEED` (`interop_resource_proof_cache.py`): as
//!   `resource-recv`, but the relay drops every resource proof toward RNS until RNS sends a
//!   cache request, so RNS has to recover the proof with one.
//! - `resource-meta LEN SEED` (`interop_resource_metadata.py`): RNS sends a Resource with
//!   metadata and `ResourceSession::receive` takes it; then a `ResourceSession` opened
//!   to RNS publishes one with `ResourceSession::publish_with_metadata`.
//! - `resource-cancel LEN SEED` (`interop_resource_cancel.py`): an accept hook rejects an
//!   RNS offer; RNS cancels a transfer mid-way; RNS rejects a Retinue publish; and a
//!   Retinue publish times out mid-way. Each side must stop promptly.
//! - `resource-segments LEN SEED` (`interop_library_resource_segments.py`): as
//!   `resource-recv` then `resource-send`, with a payload past one segment.
//! - `request-resource LEN SEED` (`interop_library_request_resource.py`): a request of `LEN`
//!   data bytes each way, too large for a packet, each answered with its echo.
//!
//! The endpoint listens behind a byte-for-byte TCP relay that tallies a deframed copy of
//! each direction by packet type and context, printed as `TAP` lines when the mode ends.

mod liveness;
mod resource;
mod segments;
mod stream;
mod support;
mod tap;

use std::sync::Arc;
use std::time::Duration;

use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use tokio::net::TcpListener;

use liveness::liveness;
use resource::{resource_cancel, resource_meta, resource_recv, resource_send};
use segments::{request_resource, resource_segments};
use stream::{stream_open, stream_respond};
use tap::{Tally, print_tally, tap};

const IDENTITY_SEED: [u8; 64] = [0x47; 64];
/// The RNS-side identity for the modes in which Retinue initiates. The Python drivers
/// build the same identity from the same 64 bytes.
const RNS_SINK_SEED: [u8; 64] = [0x5a; 64];
const RNS_STREAM_SEED: [u8; 64] = [0x5b; 64];
const RNS_LIVENESS_SEED: [u8; 64] = [0x5c; 64];
const RNS_BIG_REQUEST_SEED: [u8; 64] = [0x5d; 64];
/// RNS's link MDU at MTU 500: the largest request that travels as one packet.
const LINK_MDU: usize = 431;

/// The payload both sides agree on: a xorshift32 stream, one low byte per step.
/// Pseudo-random, so neither bz2 nor anything else shrinks it.
fn payload(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.max(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, len, seed] = args.as_slice() else {
        return Err("usage: library_oracle MODE LEN SEED".into());
    };
    let data = payload(len.parse()?, seed.parse()?);

    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &IDENTITY_SEED,
    )));
    let inner = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    println!("LISTENING {}", listener.local_addr()?.port());
    let tally: Tally = Arc::default();
    tap(
        listener,
        inner,
        tally.clone(),
        mode == "resource-recv-drop-proof",
    )
    .await?;
    // RNS's TCP client drops a peer whose first frame beats its own connection setup.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let run = match mode.as_str() {
        "resource-recv" | "resource-recv-drop-proof" => {
            resource_recv(Arc::clone(&endpoint), data).await
        }
        "resource-meta" => resource_meta(Arc::clone(&endpoint), data).await,
        "resource-cancel" => resource_cancel(Arc::clone(&endpoint), data).await,
        "resource-send" => resource_send(Arc::clone(&endpoint), data).await,
        "resource-segments" => resource_segments(Arc::clone(&endpoint), data).await,
        "request-resource" => request_resource(Arc::clone(&endpoint), data).await,
        "stream-respond" => stream_respond(Arc::clone(&endpoint), data).await,
        "stream-open" => stream_open(Arc::clone(&endpoint), data).await,
        "liveness" => {
            let hold = Duration::from_secs(len.parse()?);
            liveness(
                Arc::clone(&endpoint),
                hold,
                payload(LINK_MDU, seed.parse()?),
            )
            .await
        }
        other => Err(format!("unknown mode {other}")),
    };
    if let Err(error) = &run {
        println!("MODE_ERR {error}");
    }
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.shutdown(Duration::from_secs(3)),
    )
    .await;
    // Give the relay a moment to see the last frames before reporting.
    tokio::time::sleep(Duration::from_millis(200)).await;
    print_tally(&tally);
    println!("DONE");
    run.map_err(Into::into)
}
