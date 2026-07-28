//! Black-box capture: stock LXMF sends one opportunistic message to Retinue.
//!
//! The plaintext surfaced here is observed before Outrider has an opportunistic delivery
//! implementation. Existing codec code only checks whether the captured object is the same
//! signed LXMF grammar already proven for direct delivery.

use std::sync::Arc;
use std::time::Duration;

use outrider::{DeliveryAnnounce, delivery_destination, delivery_name};
use retinue::endpoint::Endpoint;
use retinue::identity::{KEY_LEN, PrivateIdentity};
use retinue::ratchet::{RatchetPolicy, RatchetStore};

const RECEIVER_SEED: [u8; 64] = [0x66; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = PrivateIdentity::from_secret_bytes(&RECEIVER_SEED);
    let endpoint = Arc::new(Endpoint::new(identity));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;

    let announce = DeliveryAnnounce {
        display_name: Some(b"Outrider Opportunistic Oracle".to_vec()),
        stamp_cost: None,
    };
    let app_data = announce.encode()?;
    let name = delivery_name();
    let destination = name.destination_hash(endpoint.identity());
    let mut ratchets = RatchetStore::new(RatchetPolicy::default())?;
    ratchets.rotate_if_due([0x51; KEY_LEN], 0.0)?;
    endpoint.register_resource_with_ratchets(name.clone(), &app_data, &ratchets)?;

    println!("LISTENING {}", address.port());
    println!("DESTINATION {destination}");
    println!(
        "RATCHET {}",
        ratchets.current_id().expect("current ratchet")
    );

    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move {
            loop {
                endpoint.announce(&name, &app_data);
                tokio::time::sleep(Duration::from_millis(1_100)).await;
            }
        }
    });

    let received = tokio::time::timeout(Duration::from_secs(45), endpoint.accept_single())
        .await
        .map_err(|_| "timed out waiting for stock opportunistic packet")??;
    announcer.abort();

    println!("SINGLE_PAYLOAD_LEN {}", received.data.len());
    println!("SINGLE_PAYLOAD {}", hex::encode(&received.data));
    let mut packed = Vec::with_capacity(16 + received.data.len());
    packed.extend_from_slice(received.destination.as_slice());
    packed.extend_from_slice(&received.data);
    let message = outrider::decode(&packed)?;
    if message.destination != *destination.as_bytes() {
        return Err("captured LXMF object names another destination".into());
    }
    let source = retinue::AddressHash::from_bytes(message.source);
    let source_identity = endpoint
        .resolve(source)
        .ok_or("source announce did not arrive")?;
    if source != delivery_destination(&source_identity)
        || !message.verify_with(|bytes, signature| source_identity.verify(bytes, signature))
    {
        return Err("captured LXMF signature did not verify".into());
    }

    println!("MESSAGE_ID {}", hex::encode(message.message_id));
    println!("SOURCE {}", hex::encode(message.source));
    println!("TITLE {}", hex::encode(&message.payload.title));
    println!("CONTENT {}", hex::encode(&message.payload.content));
    println!(
        "USED_RATCHET {}",
        received.ratchet_id.expect("ratchet authenticated")
    );
    println!("SIGNATURE_VERIFIED true");
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
