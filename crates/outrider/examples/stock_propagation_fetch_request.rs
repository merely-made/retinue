//! Capture one stock LXMF propagation fetch request through Retinue.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use outrider::{
    PROPAGATION_METADATA_NAME, PropagationAnnounce, PropagationCosts, announce_propagation,
    register_propagation,
};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use rmpv::Value;

const NODE_SEED: [u8; 64] = [0x70; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &NODE_SEED,
    )));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    let announce = PropagationAnnounce {
        legacy: false,
        unix_time: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
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
            Value::Binary(b"Outrider Fetch Capture".to_vec()),
        )],
    };
    let destination = register_propagation(&endpoint, &announce)?;
    println!("LISTENING {}", address.port());
    println!("PROPAGATION_DESTINATION {destination}");

    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let announce = announce.clone();
        async move {
            loop {
                announce_propagation(&endpoint, &announce).expect("fixed announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });
    let mut accepted =
        tokio::time::timeout(Duration::from_secs(180), endpoint.accept_resource()).await??;
    let request = accepted.session.receive_raw_request().await?;
    println!("REQUEST_ID {}", request.request_id);
    println!("REQUEST_PACKED {}", hex::encode(&request.packed));
    let value = rmpv::decode::read_value(&mut std::io::Cursor::new(&request.packed))?;
    println!("REQUEST_DECODED {value:?}");
    let mut offer = Vec::new();
    rmpv::encode::write_value(
        &mut offer,
        &Value::Array(vec![Value::Binary(vec![0x44; 32])]),
    )?;
    accepted.session.respond_value(request.request_id, &offer);
    let followup = accepted.session.receive_raw_request().await?;
    announcer.abort();
    println!("FOLLOWUP_ID {}", followup.request_id);
    println!("FOLLOWUP_PACKED {}", hex::encode(&followup.packed));
    let value = rmpv::decode::read_value(&mut std::io::Cursor::new(&followup.packed))?;
    println!("FOLLOWUP_DECODED {value:?}");
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
