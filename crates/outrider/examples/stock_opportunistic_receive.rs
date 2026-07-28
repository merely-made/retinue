//! Stock LXMF sends opportunistically; Outrider receives and authenticates it.

use std::sync::Arc;
use std::time::Duration;

use outrider::{DeliveryAnnounce, receive_opportunistic_with_stamp_cost, register_opportunistic};
use retinue::endpoint::Endpoint;
use retinue::identity::{KEY_LEN, PrivateIdentity};
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
    let mut ratchets = RatchetStore::new(RatchetPolicy::default())?;
    ratchets.rotate_if_due([0x51; KEY_LEN], 0.0)?;
    let destination = register_opportunistic(&endpoint, &delivery_announce, &ratchets)?;

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

    let single = tokio::time::timeout(Duration::from_secs(45), endpoint.accept_single())
        .await
        .map_err(|_| "timed out waiting for stock opportunistic delivery")??;
    let received = receive_opportunistic_with_stamp_cost(
        &endpoint,
        single,
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
        None,
    )?;
    announcer.abort();
    announcement_log.abort();

    println!("PACKED {}", hex::encode(&received.packed));
    println!("MESSAGE_ID {}", hex::encode(received.message.message_id));
    println!("TITLE {}", hex::encode(&received.message.payload.title));
    println!("CONTENT {}", hex::encode(&received.message.payload.content));
    println!("USED_RATCHET {}", received.ratchet_id);
    println!("SIGNATURE_VERIFIED true");
    println!("STAMP_POLICY none");
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
