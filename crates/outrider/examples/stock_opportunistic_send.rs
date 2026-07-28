//! Outrider sends opportunistically; stock LXMF receives through its delivery callback.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, delivery_destination, register_opportunistic, send_opportunistic,
};
use retinue::endpoint::Endpoint;
use retinue::identity::{KEY_LEN, PrivateIdentity};
use retinue::ratchet::{RatchetPolicy, RatchetStore};

const SENDER_SEED: [u8; 64] = [0x55; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = PrivateIdentity::from_secret_bytes(&SENDER_SEED);
    let endpoint = Arc::new(Endpoint::new(identity.clone()));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;

    let announce = DeliveryAnnounce {
        display_name: Some(b"Outrider Opportunistic Sender".to_vec()),
        stamp_cost: None,
    };
    let mut ratchets = RatchetStore::new(RatchetPolicy::default())?;
    ratchets.rotate_if_due([0x54; KEY_LEN], 0.0)?;
    register_opportunistic(&endpoint, &announce, &ratchets)?;
    println!("LISTENING {}", address.port());

    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let announce = announce.clone();
        async move {
            loop {
                outrider::announce_delivery(&endpoint, &announce)
                    .expect("delivery announce encodes");
                tokio::time::sleep(Duration::from_millis(1_100)).await;
            }
        }
    });

    let peer = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let peer = endpoint.next_announcement().await?;
            if peer.destination == delivery_destination(&peer.identity) {
                return Ok::<_, std::io::Error>(peer);
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for stock delivery announce")??;
    println!(
        "STOCK_ANNOUNCE {} {}",
        peer.destination,
        hex::encode(&peer.app_data)
    );

    let payload = LxmfPayload::text(
        1_753_603_212.5,
        b"OUTRIDER OPPORTUNISTIC TITLE",
        b"OUTRIDER OPPORTUNISTIC BODY",
    );
    let receipt = send_opportunistic(&endpoint, &identity, &peer, &payload)?;
    println!("MESSAGE_ID {}", hex::encode(receipt.message_id));
    println!("RATCHET {}", receipt.ratchet_id);
    println!("QUEUED {}", receipt.queued_interfaces);

    announcer.abort();
    endpoint.shutdown(Duration::from_secs(3)).await;
    Ok(())
}
