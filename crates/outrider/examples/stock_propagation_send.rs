//! Black-box propagation oracle: Outrider submits, stock lxmd stores and serves.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, PropagationAnnounce, PropagationBatch, announce_delivery,
    prepare_propagation, register_delivery, submit_propagation,
};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;

const SENDER_SEED: [u8; 64] = [0x61; 64];
const RECEIVER_SEED: [u8; 64] = [0x62; 64];
const TIMESTAMP: f64 = 1_753_603_204.5;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = PrivateIdentity::from_secret_bytes(&SENDER_SEED);
    let recipient = PrivateIdentity::from_secret_bytes(&RECEIVER_SEED);
    let endpoint = Arc::new(Endpoint::new(sender.clone()));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    endpoint.enable_routing();
    println!("LISTENING {}", address.port());

    let delivery_announce = DeliveryAnnounce::named(b"Outrider Propagation Sender");
    register_delivery(&endpoint, &delivery_announce)?;
    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let delivery_announce = delivery_announce.clone();
        async move {
            loop {
                announce_delivery(&endpoint, &delivery_announce)
                    .expect("fixed delivery announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });

    let node = loop {
        let candidate = tokio::time::timeout(Duration::from_secs(60), endpoint.next_announcement())
            .await
            .map_err(|_| "timed out waiting for stock propagation announce")??;
        if let Ok(announce) = PropagationAnnounce::decode(&candidate.app_data) {
            println!(
                "STOCK_PROPAGATION_ANNOUNCE {} cost={}",
                candidate.destination, announce.costs.propagation
            );
            break (candidate, announce);
        }
    };

    let content = if std::env::var("OUTRIDER_LARGE").is_ok_and(|value| value == "1") {
        (0..4_096_u32)
            .map(|value| value.wrapping_mul(73).wrapping_add(19) as u8)
            .collect()
    } else {
        b"PROPAGATION BODY".to_vec()
    };
    let prepared = prepare_propagation(
        &sender,
        recipient.public(),
        &LxmfPayload::text(TIMESTAMP, b"PROPAGATION TITLE", content),
        &[0x31; 32],
        &[0x41; 16],
        [0; 32],
        u16::from(node.1.costs.propagation),
        1_000_000,
    )?;
    let batch = PropagationBatch {
        transfer_time: TIMESTAMP + 0.5,
        entries: vec![prepared.entry],
    };
    let receipt = submit_propagation(&endpoint, &node.0, &batch).await?;
    println!("SUBMISSION_MODE {:?}", receipt.mode);
    println!("MESSAGE_ID {}", hex::encode(prepared.message_id));
    println!("TRANSIENT_ID {}", hex::encode(prepared.transient_id));
    println!("STAMP_VALUE {}", prepared.stamp_value);
    if std::env::var("OUTRIDER_SUMMARY").is_ok_and(|value| value == "1") {
        println!("SUBMITTED true");
        println!("SUBMISSION_LEN {}", receipt.packed_batch.len());
    } else {
        println!("SUBMITTED {}", hex::encode(&receipt.packed_batch));
    }
    announcer.abort();

    // Keep the routing endpoint up while the stock recipient fetches from lxmd.
    let linger = if std::env::var("OUTRIDER_LARGE").is_ok_and(|value| value == "1") {
        180
    } else {
        60
    };
    tokio::time::sleep(Duration::from_secs(linger)).await;
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
