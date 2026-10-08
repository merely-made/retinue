//! The retinue half of the single-packet proof gate.
//!
//! retinue registers `retinue.single` with `ProofStrategy::All` and announces it, so stock
//! RNS can send it a packet and expect a delivery proof. When the RNS destination
//! `retinue.rnsrecv` announces (proving everything, no ratchets), retinue sends it one single
//! packet, which falls back to the destination's identity key, and waits for RNS's proof on
//! the receipt.
//!
//! Driven by `oracle/interop_single_proof.py`.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ProofStrategy, SingleDelivery};
use retinue::identity::PrivateIdentity;

const IDENTITY_SEED: [u8; 64] = [0x5A; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&IDENTITY_SEED));
    let addr = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    println!("LISTENING {}", addr.port());

    let name = DestinationName::new("retinue", ["single"]);
    endpoint.register(name.clone(), b"single");
    endpoint.set_proof_strategy(&name, ProofStrategy::All)?;

    // Announce until RNS's destination has been heard. RNS may attach after the first
    // announces, so keep repeating rather than announcing once.
    let rns_name = DestinationName::new("retinue", ["rnsrecv"]);
    let rns = loop {
        endpoint.announce(&name, b"single");
        match tokio::time::timeout(Duration::from_millis(700), endpoint.next_announcement()).await {
            Ok(announce) => {
                let announce = announce?;
                if announce.destination == rns_name.destination_hash(&announce.identity) {
                    break announce;
                }
            }
            Err(_) => continue,
        }
    };
    println!("RNS_ANNOUNCE {}", rns.destination);

    // Keep announcing until RNS's packet arrives: our earliest announces can reach RNS
    // before its interface is ready, and it sends only once it has heard one.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        endpoint.announce(&name, b"single");
        if let Ok(single) =
            tokio::time::timeout(Duration::from_secs(1), endpoint.accept_single()).await
        {
            let single = single?;
            println!(
                "RECEIVED {} ratchet={}",
                String::from_utf8_lossy(&single.data),
                single
                    .ratchet_id
                    .map_or_else(|| "none".to_string(), |id| id.to_string())
            );
            break;
        }
    }

    let receipt = endpoint.send_single(rns.destination, b"from-retinue")?;
    println!(
        "SENT ratchet={}",
        receipt
            .ratchet_id
            .map_or_else(|| "none".to_string(), |id| id.to_string())
    );
    match receipt.delivery().await {
        SingleDelivery::Delivered { rtt } => {
            println!("RECEIPT DELIVERED rtt_ms={}", rtt.as_millis())
        }
        other => println!("RECEIPT {other:?}"),
    }
    // Leave our proof of RNS's packet time to cross the socket before the gate checks it.
    tokio::time::sleep(Duration::from_secs(1)).await;

    println!("DONE");
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
