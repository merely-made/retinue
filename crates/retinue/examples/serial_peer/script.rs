//! What Retinue does in the serial and RNode gates, whichever carrier it rides.
//!
//! It announces `retinue.serial-peer` until a link arrives, echoes one request, takes one
//! Resource, and optionally publishes one to stock RNS's `retinue.serial-sink`. Markers on
//! stdout tell the Python driver what happened; the driver judges by stock's own state.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use retinue::Ifac;
use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ReceivedPayload, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::request::Request;

pub const IDENTITY_SEED: [u8; 64] = [0x63; 64];
/// Stock RNS's sink identity; the Python driver builds it from the same bytes.
pub const RNS_SINK_SEED: [u8; 64] = [0x64; 64];
const NETWORK_NAME: &str = "retinue-serial-gate";
const PASSPHRASE: &str = "serial-and-radio";
const WAIT: Duration = Duration::from_secs(180);

pub fn endpoint() -> Arc<Endpoint> {
    Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &IDENTITY_SEED,
    )))
}

/// The gate's IFAC credentials at `bits` (an RNS `ifac_size`).
pub fn ifac(bits: Option<usize>) -> Result<Option<Ifac>, String> {
    bits.map(|bits| Ifac::new(Some(NETWORK_NAME), Some(PASSPHRASE), bits / 8))
        .transpose()
        .map_err(|e| format!("ifac: {e:?}"))
}

/// A xorshift32 stream, one low byte per step, matching the driver's.
pub fn payload(len: usize, seed: u32) -> Vec<u8> {
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

/// Count every validated announce, for the flood check.
pub fn count_announces(endpoint: &Arc<Endpoint>) -> Arc<AtomicU64> {
    let count = Arc::new(AtomicU64::new(0));
    let (ep, seen) = (Arc::clone(endpoint), Arc::clone(&count));
    tokio::spawn(async move {
        while ep.next_announcement().await.is_ok() {
            seen.fetch_add(1, Ordering::Relaxed);
        }
    });
    count
}

/// Announce `count` destinations back to back, then wait for stdin to close.
pub async fn burst(endpoint: Arc<Endpoint>, count: usize) {
    for n in 0..count {
        endpoint.announce(
            &DestinationName::new("retinue", [format!("burst-{n}")]),
            b"",
        );
    }
    println!("BURST {count}");
    until_stdin_closes().await;
}

/// Serve one link, then publish `publish` bytes to stock if asked, then idle until stdin
/// closes.
pub async fn serve(endpoint: Arc<Endpoint>, publish: Option<usize>) {
    let announces = count_announces(&endpoint);
    if let Err(error) = serve_link(&endpoint).await {
        println!("SERVE_ERR {error}");
    }
    if let Some(len) = publish
        && let Err(error) = publish_to_stock(&endpoint, payload(len, 0x5E21_0002)).await
    {
        println!("PUBLISH_ERR {error}");
    }
    until_stdin_closes().await;
    let held = endpoint.routing_counters().held_announces;
    println!(
        "ANNOUNCES {} HELD {held}",
        announces.load(Ordering::Relaxed)
    );
}

async fn serve_link(endpoint: &Arc<Endpoint>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["serial-peer"]);
    endpoint.register_resource(name.clone(), b"serial-peer");
    println!("DEST {}", name.destination_hash(endpoint.identity()));
    let announcing = {
        let (ep, name) = (Arc::clone(endpoint), name.clone());
        tokio::spawn(async move {
            loop {
                ep.announce(&name, b"serial-peer");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        })
    };
    let accepted = tokio::time::timeout(WAIT, endpoint.accept_resource()).await;
    announcing.abort();
    let mut session = accepted
        .map_err(|_| "no link")?
        .map_err(|e| format!("accept: {e}"))?
        .session;
    println!("LINK {}", session.link_id());
    session.set_config(ResourceTransferConfig {
        timeout: WAIT,
        ..ResourceTransferConfig::default()
    });

    let request = session
        .receive_raw_request()
        .await
        .map_err(|e| format!("request: {e}"))?;
    let data = Request::unpack(&request.packed)
        .map(|r| r.data)
        .unwrap_or_default();
    session
        .respond_auto(request.request_id, data)
        .await
        .map_err(|e| format!("respond: {e}"))?;
    println!("REQUEST_OK");

    match session.receive().await {
        Ok(ReceivedPayload::Resource(data)) => {
            let ok = data == payload(data.len(), 0x5E21_0001);
            println!(
                "RESOURCE {} {}",
                data.len(),
                if ok { "OK" } else { "MISMATCH" }
            );
        }
        Ok(other) => println!("UNEXPECTED {other:?}"),
        Err(e) => println!("RESOURCE_ERR {e}"),
    }
    // Hold the link until stock hangs up, so its proof lands before any close.
    let _ = tokio::time::timeout(Duration::from_secs(30), session.receive()).await;
    Ok(())
}

async fn publish_to_stock(endpoint: &Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
    let sink = DestinationName::new("retinue", ["serial-sink"]);
    let dest = sink.destination_hash(PrivateIdentity::from_secret_bytes(&RNS_SINK_SEED).public());
    let deadline = tokio::time::Instant::now() + WAIT;
    let identity = loop {
        if let Some(identity) = endpoint.resolve(dest) {
            break identity;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("stock sink never announced".into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let mut session = tokio::time::timeout(WAIT, endpoint.open_resource(dest, identity))
        .await
        .map_err(|_| "open_resource timed out")?
        .map_err(|e| format!("open_resource: {e}"))?;
    session.set_config(ResourceTransferConfig {
        timeout: WAIT,
        ..ResourceTransferConfig::default()
    });
    session.publish(&data).await.map_err(|e| format!("{e}"))?;
    println!("PUBLISH_OK {}", data.len());
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}

pub async fn until_stdin_closes() {
    let _ = tokio::task::spawn_blocking(|| {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
    })
    .await;
}
