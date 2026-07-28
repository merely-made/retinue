//! Black-box direct-delivery oracle: Outrider sends, stock LXMF receives.
//!
//! The Python driver under `oracle/` connects pinned RNS and LXMF packages to
//! this endpoint and judges the resulting message through stock LXMF's public
//! delivery callback.

use std::sync::Arc;
use std::time::Duration;

use outrider::DeliveryAnnounce;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use sha2::{Digest, Sha256};

const SENDER_SEED: [u8; 64] = [0x55; 64];
const TIMESTAMP: f64 = 1_753_603_202.5;

fn large_content() -> Vec<u8> {
    (0..128_u32)
        .flat_map(|value| Sha256::digest(value.to_be_bytes()).to_vec())
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = PrivateIdentity::from_secret_bytes(&SENDER_SEED);
    let endpoint = Arc::new(Endpoint::new(identity.clone()));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    println!("LISTENING {}", address.port());

    let source_announce = DeliveryAnnounce::named(b"Outrider Oracle");
    let source = outrider::register_delivery(&endpoint, &source_announce)?;
    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let source_announce = source_announce.clone();
        async move {
            loop {
                outrider::announce_delivery(&endpoint, &source_announce)
                    .expect("delivery announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });

    let stock = tokio::time::timeout(Duration::from_secs(30), endpoint.next_announcement())
        .await
        .map_err(|_| "timed out waiting for stock LXMF delivery announce")??;
    println!(
        "STOCK_ANNOUNCE {} {}",
        stock.destination,
        hex::encode(&stock.app_data)
    );

    let large = std::env::var("OUTRIDER_LARGE").is_ok_and(|value| value == "1");
    let title = if large {
        b"OUTRIDER LARGE TITLE".as_slice()
    } else {
        b"OUTRIDER TITLE".as_slice()
    };
    let content = if large {
        large_content()
    } else {
        b"OUTRIDER BODY".to_vec()
    };
    let payload = outrider::LxmfPayload::text(TIMESTAMP, title, content);
    let receipt =
        outrider::send_direct_stamped(&endpoint, &identity, &stock, &payload, [0; 32], 100_000)
            .await?;
    announcer.abort();

    println!("SENT {}", receipt.packed.len());
    println!("TRANSPORT {:?}", receipt.mode);
    println!("PACKED {}", hex::encode(&receipt.packed));
    println!("MESSAGE_ID {}", hex::encode(receipt.message_id));
    println!("SOURCE {source}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
