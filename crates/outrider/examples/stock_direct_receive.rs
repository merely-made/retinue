//! Black-box direct-delivery oracle: stock LXMF sends, Outrider receives.
//!
//! The Python driver under `oracle/` starts this process, connects pinned RNS
//! and LXMF packages over a TCP interface, and sends one ordinary direct
//! message. This example prints the captured LXMF object and verifies it only
//! through Outrider and Retinue's public boundaries.

use std::sync::Arc;
use std::time::Duration;

use outrider::DeliveryAnnounce;
use retinue::endpoint::{Endpoint, PayloadMode};
use retinue::identity::PrivateIdentity;

const RECEIVER_SEED: [u8; 64] = [0x66; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = PrivateIdentity::from_secret_bytes(&RECEIVER_SEED);
    let endpoint = Arc::new(Endpoint::new(identity.clone()));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    let delivery_announce = DeliveryAnnounce {
        display_name: Some(b"Outrider Oracle".to_vec()),
        stamp_cost: Some(8),
    };
    let destination = outrider::register_delivery(&endpoint, &delivery_announce)?;

    println!("LISTENING {}", address.port());
    println!("DESTINATION {destination}");

    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let delivery_announce = delivery_announce.clone();
        async move {
            loop {
                outrider::announce_delivery(&endpoint, &delivery_announce)
                    .expect("delivery announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });
    let announcement_log = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move {
            while let Ok(announcement) = endpoint.next_announcement().await {
                println!(
                    "ANNOUNCE {} {} {}",
                    announcement.destination,
                    hex::encode(announcement.identity.to_public_bytes()),
                    hex::encode(announcement.app_data)
                );
            }
        }
    });

    let accepted = tokio::time::timeout(Duration::from_secs(30), endpoint.accept_resource())
        .await
        .map_err(|_| "timed out waiting for stock LXMF direct delivery")??;
    let received = outrider::receive_direct_with_stamp_cost(
        &endpoint,
        accepted,
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
        Some(8),
    )
    .await?;
    let transport = match received.mode {
        PayloadMode::Data => "data",
        PayloadMode::Resource => "resource",
    };
    announcer.abort();
    announcement_log.abort();

    println!("RECEIVED {}", received.packed.len());
    println!("TRANSPORT {transport}");
    println!("PACKED {}", hex::encode(&received.packed));
    println!("MESSAGE_ID {}", hex::encode(received.message.message_id));
    println!(
        "SOURCE {}",
        outrider::delivery_destination(&received.source_identity)
    );
    println!("TITLE {}", hex::encode(&received.message.payload.title));
    println!("CONTENT {}", hex::encode(&received.message.payload.content));
    println!("SIGNATURE_VERIFIED true");

    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
