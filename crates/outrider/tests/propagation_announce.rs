//! The node announce carries the node's clock at each announce and path response
//! (`LXMRouter.py` 193, 332-346), and a re-announce becomes what path responses carry.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use outrider::{PropagationAnnounce, PropagationCosts, announce_propagation, register_propagation};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn next(client: &Endpoint) -> PropagationAnnounce {
    let heard = tokio::time::timeout(Duration::from_secs(2), client.next_announcement())
        .await
        .unwrap()
        .unwrap();
    PropagationAnnounce::decode(&heard.app_data).unwrap()
}

#[tokio::test]
async fn announces_and_path_responses_carry_a_current_timebase() {
    let client = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x81; 64],
    )));
    let node = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x82; 64],
    )));
    connect(&client, &node, LossModel::new(81), LossModel::new(82));
    let mut announce = PropagationAnnounce {
        legacy: false,
        unix_time: 1,
        active: true,
        transfer_limit_kb: 256.0,
        sync_limit_kb: 10_240.0,
        costs: PropagationCosts {
            propagation: 16,
            flexibility: 3,
            peering: 18,
        },
        metadata: Vec::new(),
    };
    let started = now();
    let destination = register_propagation(&node, &announce).unwrap();
    let heard = next(&client).await;
    assert!(heard.unix_time >= started, "registration announces now");

    announce.active = false;
    announce_propagation(&node, &announce).unwrap();
    let heard = next(&client).await;
    assert!(!heard.active && heard.unix_time >= started);

    client.request_path(destination);
    let answered = next(&client).await;
    assert!(!answered.active, "path responses carry the latest announce");
    assert!(answered.unix_time >= started);
}
