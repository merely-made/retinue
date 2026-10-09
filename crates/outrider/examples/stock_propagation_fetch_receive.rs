//! Capture one stock lxmd propagation fetch response through Retinue.

use std::io;
use std::time::Duration;

use outrider::{
    Acknowledgement, DeliveryAnnounce, FetchPolicy, PropagationAnnounce, Verification,
    announce_delivery, fetch_propagation, propagation_destination, register_opportunistic,
};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::ratchet::RatchetStore;

const RECEIVER_SEED: [u8; 64] = [0x62; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address: std::net::SocketAddr = std::env::var("RETINUE_ADDR")?.parse()?;
    let identity = PrivateIdentity::from_secret_bytes(&RECEIVER_SEED);
    let endpoint = Endpoint::new(identity.clone());
    endpoint.attach_tcp_client(address).await?;
    let delivery = DeliveryAnnounce::named(b"Outrider Propagation Receiver");
    register_opportunistic(&endpoint, &delivery, RatchetStore::new(Default::default())?)?;
    for _ in 0..3 {
        announce_delivery(&endpoint, &delivery)?;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let node = loop {
        let candidate = tokio::time::timeout(Duration::from_secs(60), endpoint.next_announcement())
            .await
            .map_err(|_| "timed out waiting for stock propagation announce")??;
        if PropagationAnnounce::decode(&candidate.app_data).is_ok()
            && candidate.destination == propagation_destination(&candidate.identity)
        {
            break candidate;
        }
    };
    println!("FETCH_READY {}", node.destination);
    let mut trigger = String::new();
    io::stdin().read_line(&mut trigger)?;

    let policy = FetchPolicy {
        max_messages: 1,
        ..FetchPolicy::default()
    };
    let receipt = fetch_propagation(&endpoint, &node, 1_785_206_500.5, |_| false, &policy).await?;
    println!("OFFERED {}", receipt.offered.len());
    println!("FETCHED {}", receipt.messages.len());
    for fetched in &receipt.messages {
        println!("TRANSIENT_ID {}", hex::encode(fetched.transient_id));
        println!("MESSAGE_ID {}", hex::encode(fetched.message.message_id));
        println!("TITLE {}", hex::encode(&fetched.message.payload.title));
        let ratchet = fetched
            .ratchet_id
            .map_or_else(|| "none".into(), |id| hex::encode(id.as_slice()));
        println!("RATCHET_ID {ratchet}");
        println!(
            "VERIFIED {}",
            matches!(fetched.verification, Verification::Verified(_))
        );
        if std::env::var("OUTRIDER_SUMMARY").is_ok_and(|value| value == "1") {
            println!("CONTENT_LEN {}", fetched.message.payload.content.len());
        } else {
            println!("CONTENT {}", hex::encode(&fetched.message.payload.content));
        }
    }
    for rejected in &receipt.rejected {
        println!(
            "REJECTED {} {}",
            hex::encode(rejected.transient_id),
            rejected.error
        );
    }
    println!(
        "ACKNOWLEDGED {}",
        matches!(receipt.acknowledgement, Acknowledgement::Confirmed)
    );
    println!(
        "PRODUCTION_FETCH {}",
        receipt.offered.len() == 1 && receipt.messages.len() == 1
    );
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
