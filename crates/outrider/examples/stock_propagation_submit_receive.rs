//! Capture one stock LXMF propagation submission through Retinue.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use outrider::{
    PROPAGATION_METADATA_NAME, PropagationAnnounce, PropagationCosts, announce_propagation,
    propagation_destination, receive_submission, register_propagation,
};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use rmpv::Value;

const NODE_SEED: [u8; 64] = [0x70; 64];
const RECEIVER_SEED: [u8; 64] = [0x62; 64];
const SENDER_SEED: [u8; 64] = [0x61; 64];

fn propagation_announce() -> PropagationAnnounce {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs();
    PropagationAnnounce {
        legacy: false,
        unix_time: started,
        active: true,
        transfer_limit_kib: 256,
        sync_limit_kib: 10_240,
        costs: PropagationCosts {
            propagation: 13,
            flexibility: 3,
            peering: 8,
        },
        metadata: vec![(
            Value::from(PROPAGATION_METADATA_NAME),
            Value::Binary(b"Outrider Capture Node".to_vec()),
        )],
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &NODE_SEED,
    )));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    let announce = propagation_announce();
    let destination = register_propagation(&endpoint, &announce)?;
    let app_data = announce.encode()?;

    println!("LISTENING {}", address.port());
    println!("PROPAGATION_DESTINATION {destination}");
    println!("PROPAGATION_APP_DATA {}", hex::encode(&app_data));

    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let announce = announce.clone();
        async move {
            loop {
                announce_propagation(&endpoint, &announce).expect("valid fixed announce");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });

    let accepted =
        tokio::time::timeout(Duration::from_secs(180), endpoint.accept_resource()).await??;
    let received = receive_submission(&endpoint, accepted, 13, 16 * 1024 * 1024, 4_096).await?;
    announcer.abort();
    println!("SUBMISSION_MODE {:?}", received.mode);
    println!("SUBMISSION_LEN {}", received.packed_batch.len());
    println!("SUBMISSION {}", hex::encode(&received.packed_batch));
    println!("SUBMISSION_DECODED {:?}", received.batch);
    let entry = received
        .batch
        .entries
        .first()
        .ok_or("stock submitted an empty propagation batch")?;
    println!("MESSAGE_DESTINATION {}", hex::encode(entry.destination()));
    println!("PROPAGATION_STAMP {}", hex::encode(entry.stamp()));
    println!("TRANSIENT_ID {}", hex::encode(entry.transient_id()));
    println!("STAMP_VALUE {}", entry.stamp_value());
    let recipient = PrivateIdentity::from_secret_bytes(&RECEIVER_SEED);
    let sender = PrivateIdentity::from_secret_bytes(&SENDER_SEED);
    let decoded = entry.decrypt_and_verify(
        &recipient,
        sender.public(),
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
    )?;
    println!("DECRYPTED_MESSAGE_ID {}", hex::encode(decoded.message_id));
    println!("DECRYPTED_TITLE {}", hex::encode(&decoded.payload.title));
    println!(
        "DECRYPTED_CONTENT {}",
        hex::encode(&decoded.payload.content)
    );
    println!(
        "PRODUCTION_RECEIVE {}",
        propagation_destination(endpoint.identity()) == destination
    );
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
