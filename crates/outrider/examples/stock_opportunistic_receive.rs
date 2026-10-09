//! Stock LXMF sends opportunistically; Outrider receives and authenticates it.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveredCache, DeliveryAnnounce, OpportunisticError, Verification,
    receive_opportunistic_with_stamp_cost, register_delivery, register_opportunistic,
};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::ratchet::{RatchetPolicy, RatchetStore};

const RECEIVER_SEED: [u8; 64] = [0x66; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &RECEIVER_SEED,
    )));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    let delivery_announce = DeliveryAnnounce {
        display_name: Some(b"Outrider Opportunistic Receiver".to_vec()),
        stamp_cost: None,
    };
    // `OUTRIDER_NO_RATCHETS=1` registers without ratchets, so stock encrypts to the identity.
    let destination = if std::env::var_os("OUTRIDER_NO_RATCHETS").is_some_and(|v| v == "1") {
        register_delivery(&endpoint, &delivery_announce)?
    } else {
        let ratchets = RatchetStore::new(RatchetPolicy::default())?;
        register_opportunistic(&endpoint, &delivery_announce, ratchets)?
    };

    println!("LISTENING {}", address.port());
    println!("DESTINATION {destination}");

    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let delivery_announce = delivery_announce.clone();
        async move {
            loop {
                outrider::announce_delivery(&endpoint, &delivery_announce)
                    .expect("delivery announce encodes");
                tokio::time::sleep(Duration::from_millis(1_100)).await;
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

    // Receive until one message and `OUTRIDER_EXPECT_DUPLICATES` resends of it (default 0)
    // have been handled. Each verified packet is proved, a duplicate included.
    let expect_duplicates: usize = std::env::var("OUTRIDER_EXPECT_DUPLICATES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let delivered = DeliveredCache::default();
    let (mut fresh, mut duplicates) = (0, 0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while fresh == 0 || duplicates < expect_duplicates {
        let single = tokio::time::timeout_at(deadline, endpoint.accept_single())
            .await
            .map_err(|_| "timed out waiting for stock opportunistic delivery")??;
        match receive_opportunistic_with_stamp_cost(
            &endpoint,
            single,
            &delivered,
            outrider::DEFAULT_MAX_MESSAGE_BYTES,
            None,
        ) {
            Ok(received) if received.verification == Verification::Verified => {
                fresh += 1;
                println!("PACKED {}", hex::encode(&received.packed));
                println!("MESSAGE_ID {}", hex::encode(received.message.message_id));
                println!("TITLE {}", hex::encode(&received.message.payload.title));
                println!("CONTENT {}", hex::encode(&received.message.payload.content));
                match received.ratchet_id {
                    Some(ratchet_id) => println!("USED_RATCHET {ratchet_id}"),
                    None => println!("USED_RATCHET none"),
                }
                println!("SIGNATURE_VERIFIED true");
                println!("STAMP_POLICY none");
            }
            Ok(received) => println!("UNVERIFIED {}", hex::encode(received.message.message_id)),
            Err(OpportunisticError::Duplicate(id)) => {
                duplicates += 1;
                println!("DUPLICATE {}", hex::encode(id));
            }
            Err(error) => println!("REFUSED {error}"),
        }
    }
    announcer.abort();
    announcement_log.abort();
    // Let the last proof leave before the interface goes.
    tokio::time::sleep(Duration::from_millis(500)).await;
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
