//! What Retinue does in the datagram gates (`oracle/datagram_gate.py`), over UDP or Auto.
//!
//! It announces `retinue.datagram-peer` until a link arrives, takes one Resource, proves one
//! link packet, publishes one Resource back, and holds the link until stock hangs up.
//! Markers on stdout tell the driver what happened; the driver judges by stock's state.

use std::sync::Arc;
use std::time::Duration;

use retinue::Ifac;
use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ReceivedPayload, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;

const IDENTITY_SEED: [u8; 64] = [0x6d; 64];
const NETWORK_NAME: &str = "retinue-datagram-gate";
const PASSPHRASE: &str = "udp-and-auto";
const WAIT: Duration = Duration::from_secs(120);
/// Seeds of the driver's payloads: stock's Resource, its link packet, and Retinue's reply.
const RESOURCE_SEED: u32 = 0xDA7A_0001;
const PACKET_SEED: u32 = 0xDA7A_0002;
const PUBLISH_SEED: u32 = 0xDA7A_0003;
const PUBLISH_LEN: usize = 4096;

pub fn endpoint() -> Arc<Endpoint> {
    Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &IDENTITY_SEED,
    )))
}

/// The gate's credentials at stock's datagram default, 16 bytes.
pub fn ifac(on: bool) -> Option<Ifac> {
    on.then(|| Ifac::for_stream(Some(NETWORK_NAME), Some(PASSPHRASE)).expect("valid IFAC"))
}

/// A xorshift32 stream, one low byte per step, matching `oracle/pty_bridge.py`.
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

/// Serve one link until stdin closes, which ends the run at any point.
pub async fn serve(endpoint: Arc<Endpoint>) {
    let mut closed = tokio::task::spawn_blocking(|| {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
    });
    tokio::select! {
        served = serve_link(&endpoint) => {
            if let Err(error) = served {
                println!("SERVE_ERR {error}");
            }
            let _ = closed.await;
        }
        _ = &mut closed => {}
    }
}

async fn serve_link(endpoint: &Arc<Endpoint>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["datagram-peer"]);
    endpoint.register_resource(name.clone(), b"datagram-peer");
    println!("DEST {}", name.destination_hash(endpoint.identity()));
    let announcing = {
        let (ep, name) = (Arc::clone(endpoint), name.clone());
        tokio::spawn(async move {
            loop {
                ep.announce(&name, b"datagram-peer");
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

    match session.receive().await {
        Ok(ReceivedPayload::Resource(data)) => {
            let ok = data == payload(data.len(), RESOURCE_SEED);
            println!(
                "RESOURCE {} {}",
                data.len(),
                if ok { "OK" } else { "MISMATCH" }
            );
        }
        other => return Err(format!("expected a Resource: {other:?}")),
    }
    match session.receive().await {
        Ok(ReceivedPayload::Data(data)) => {
            let ok = data == payload(data.len(), PACKET_SEED);
            println!("DATA {} {}", data.len(), if ok { "OK" } else { "MISMATCH" });
            session.prove_data().map_err(|e| format!("prove: {e}"))?;
            println!("DATA_PROVED");
        }
        other => return Err(format!("expected a link packet: {other:?}")),
    }
    let back = payload(PUBLISH_LEN, PUBLISH_SEED);
    session
        .publish(&back)
        .await
        .map_err(|e| format!("publish: {e}"))?;
    println!("PUBLISH_OK {}", back.len());
    // Hold the link until stock hangs up, so its callbacks finish before any close. A
    // payload arriving now is a duplicate delivery, which the gates count as a failure.
    let hold = async {
        while let Ok(payload) = session.receive().await {
            let (ReceivedPayload::Data(bytes) | ReceivedPayload::Resource(bytes)) = payload;
            println!("DATA_AGAIN {}", bytes.len());
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(30), hold).await;
    Ok(())
}
