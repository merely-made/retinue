//! Outrider submits to stock lxmd twice: once under a stamp below the node's floor, which
//! lxmd answers with `ERROR_INVALID_STAMP`, then properly, which lxmd proves.
//!
//! The first submission works from a stale copy of the node's announce whose cost reads 0,
//! as a client holding an announce from before the node raised its cost would.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, PropagationAnnounce, PropagationBatch, PropagationError,
    PropagationSubmitReceipt, announce_delivery, prepare_propagation, register_delivery,
    submit_propagation,
};
use retinue::endpoint::{Endpoint, PeerAnnounce};
use retinue::identity::PrivateIdentity;

const SENDER_SEED: [u8; 64] = [0x61; 64];
const RECEIVER_SEED: [u8; 64] = [0x62; 64];
const TIMESTAMP: f64 = 1_753_603_230.5;

/// Prepare and submit one message. With `under` set, seeds are walked until the stamp
/// scores below it.
async fn submit(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    node: &PeerAnnounce,
    title: &[u8],
    target: u16,
    under: Option<u16>,
) -> Result<(String, Result<PropagationSubmitReceipt, PropagationError>), Box<dyn std::error::Error>>
{
    let recipient = PrivateIdentity::from_secret_bytes(&RECEIVER_SEED);
    let payload = LxmfPayload::text(TIMESTAMP, title, b"PROPAGATION BODY".as_slice());
    let (mut ephemeral, mut iv) = ([0; 32], [0; 16]);
    getrandom::fill(&mut ephemeral).map_err(|error| error.to_string())?;
    getrandom::fill(&mut iv).map_err(|error| error.to_string())?;
    let mut nonce = 0_u8;
    let prepared = loop {
        let prepared = prepare_propagation(
            sender,
            recipient.public(),
            &payload,
            &ephemeral,
            &iv,
            [nonce; 32],
            target,
            1_000_000,
        )?;
        if under.is_none_or(|floor| prepared.stamp_value < floor) {
            break prepared;
        }
        nonce += 1;
    };
    println!(
        "PREPARED {} {} {}",
        String::from_utf8_lossy(title),
        hex::encode(prepared.message_id),
        prepared.stamp_value
    );
    let batch = PropagationBatch {
        transfer_time: TIMESTAMP + 0.5,
        entries: vec![prepared.entry],
    };
    Ok((
        hex::encode(prepared.message_id),
        submit_propagation(endpoint, node, &batch).await,
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = PrivateIdentity::from_secret_bytes(&SENDER_SEED);
    let endpoint = Arc::new(Endpoint::new(sender.clone()));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    endpoint.enable_routing();
    println!("LISTENING {}", address.port());

    let delivery_announce = DeliveryAnnounce::named(b"Outrider Rejected Sender");
    register_delivery(&endpoint, &delivery_announce)?;
    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move {
            loop {
                announce_delivery(&endpoint, &delivery_announce)
                    .expect("fixed delivery announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });

    let (node, announce) = loop {
        let candidate = tokio::time::timeout(Duration::from_secs(60), endpoint.next_announcement())
            .await
            .map_err(|_| "timed out waiting for stock propagation announce")??;
        if let Ok(announce) = PropagationAnnounce::decode(&candidate.app_data) {
            break (candidate, announce);
        }
    };
    let floor = u16::from(
        announce
            .costs
            .propagation
            .saturating_sub(announce.costs.flexibility),
    );
    println!(
        "NODE {} cost={} floor={floor}",
        node.destination, announce.costs.propagation
    );

    let mut stale = announce.clone();
    stale.costs.propagation = 0;
    let stale_node = PeerAnnounce {
        app_data: stale.encode()?,
        ..node.clone()
    };
    let (rejected_id, rejected) = submit(
        &endpoint,
        &sender,
        &stale_node,
        b"REJECTED TITLE",
        0,
        Some(floor),
    )
    .await?;
    match rejected {
        Err(PropagationError::Rejected) => println!("REJECTED {rejected_id}"),
        other => println!("NOT_REJECTED {other:?}"),
    }

    let target = u16::from(announce.costs.propagation);
    let (accepted_id, accepted) =
        submit(&endpoint, &sender, &node, b"ACCEPTED TITLE", target, None).await?;
    match accepted {
        Ok(receipt) if receipt.proved => println!("PROVED {accepted_id} {:?}", receipt.mode),
        other => println!("NOT_PROVED {other:?}"),
    }
    announcer.abort();

    // Keep the routing endpoint up while the stock recipient fetches from lxmd.
    tokio::time::sleep(Duration::from_secs(60)).await;
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
