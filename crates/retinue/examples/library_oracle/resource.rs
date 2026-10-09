//! The Resource modes: receive, metadata, cancel, and send.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;

use crate::support::{
    accept_resource_link, hold_until_peer_closes, report_received, resolve_rns, transfer_config,
};
use crate::{RNS_SINK_SEED, hex};

pub(super) async fn resource_recv(
    endpoint: Arc<Endpoint>,
    expected: Vec<u8>,
) -> Result<(), String> {
    let mut session = accept_resource_link(&endpoint).await?;
    session.set_config(transfer_config(Duration::from_secs(60)));
    report_received(session.receive().await, &expected);
    hold_until_peer_closes(&mut session).await;
    Ok(())
}

/// The metadata Retinue attaches when it publishes: msgpack `{"name": "retinue.bin", "n": 7}`.
const RETINUE_METADATA: &[u8] = b"\x82\xa4name\xabretinue.bin\xa1n\x07";

pub(super) async fn resource_meta(endpoint: Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
    // RNS publishes to us with metadata.
    let mut session = accept_resource_link(&endpoint).await?;
    session.set_config(transfer_config(Duration::from_secs(60)));
    report_received(session.receive().await, &data);
    match session.take_metadata() {
        Some(metadata) => println!("METADATA {}", hex(&metadata)),
        None => println!("METADATA_NONE"),
    }
    hold_until_peer_closes(&mut session).await;
    drop(session);

    // We publish to RNS with metadata.
    let name = DestinationName::new("retinue", ["library-sink"]);
    let (dest, identity) = resolve_rns(&endpoint, &name, &RNS_SINK_SEED)
        .await
        .ok_or("RNS sink announce not seen")?;
    let mut session = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_resource(dest, identity),
    )
    .await
    .map_err(|_| "open_resource timed out".to_string())?
    .map_err(|e| format!("open_resource: {e}"))?;
    println!("SEND_LINK {}", session.link_id());
    session.set_config(transfer_config(Duration::from_secs(60)));
    match session.publish_with_metadata(&data, RETINUE_METADATA).await {
        Ok(()) => println!("PUBLISH_OK {}", data.len()),
        Err(e) => println!("PUBLISH_ERR {:?} {e}", e.kind()),
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// Four refusals, each of which must end the other side's transfer promptly.
pub(super) async fn resource_cancel(endpoint: Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
    // 1. Our accept hook rejects RNS's offer before any part moves.
    let mut session = accept_resource_link(&endpoint).await?;
    session.set_config(transfer_config(Duration::from_secs(60)));
    session.set_accept(|advertisement| {
        println!(
            "OFFER {} {} {}",
            advertisement.data_size, advertisement.transfer_size, advertisement.flags
        );
        false
    });
    match session.receive().await {
        Err(e) => println!("REJECTED_OFFER {:?}", e.kind()),
        Ok(_) => println!("REJECT_UNEXPECTED_PAYLOAD"),
    }
    hold_until_peer_closes(&mut session).await;
    drop(session);

    // 2. RNS cancels its transfer part-way; our receive ends on its cancel.
    let mut session = accept_resource_link(&endpoint).await?;
    session.set_config(transfer_config(Duration::from_secs(60)));
    let started = tokio::time::Instant::now();
    match session.receive().await {
        Err(e) => println!(
            "SENDER_CANCELED {:?} {}",
            e.kind(),
            started.elapsed().as_millis()
        ),
        Ok(_) => println!("CANCEL_UNEXPECTED_PAYLOAD"),
    }
    hold_until_peer_closes(&mut session).await;
    drop(session);

    // 3. RNS rejects our publish; it ends on the rejection, long before its timeout.
    let name = DestinationName::new("retinue", ["library-sink"]);
    let (dest, identity) = resolve_rns(&endpoint, &name, &RNS_SINK_SEED)
        .await
        .ok_or("RNS sink announce not seen")?;
    let mut session = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_resource(dest, identity),
    )
    .await
    .map_err(|_| "open_resource timed out".to_string())?
    .map_err(|e| format!("open_resource: {e}"))?;
    println!("SEND_LINK {}", session.link_id());
    session.set_config(transfer_config(Duration::from_secs(60)));
    let started = tokio::time::Instant::now();
    match session.publish(&data).await {
        Ok(()) => println!("PUBLISH_UNEXPECTED_OK"),
        Err(e) => println!(
            "PUBLISH_REJECTED {:?} {}",
            e.kind(),
            started.elapsed().as_millis()
        ),
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(session);

    // 4. Our publish times out part-way; RNS's receive ends on our cancel.
    let mut session = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_resource(dest, identity),
    )
    .await
    .map_err(|_| "open_resource timed out".to_string())?
    .map_err(|e| format!("open_resource: {e}"))?;
    println!("SEND_LINK {}", session.link_id());
    session.set_config(transfer_config(Duration::from_millis(400)));
    match session.publish(&data).await {
        Ok(()) => println!("PUBLISH_UNEXPECTED_OK"),
        Err(e) => println!("PUBLISH_GAVE_UP {:?}", e.kind()),
    }
    // Hold the link well past the gate's promptness bound, so RNS's receive can only end
    // that soon on our cancel, not on the link closing.
    tokio::time::sleep(Duration::from_secs(8)).await;
    Ok(())
}

pub(super) async fn resource_send(endpoint: Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["library-sink"]);
    let (dest, identity) = resolve_rns(&endpoint, &name, &RNS_SINK_SEED)
        .await
        .ok_or("RNS sink announce not seen")?;
    println!("RESOLVED {dest}");
    let mut session = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_resource(dest, identity),
    )
    .await
    .map_err(|_| "open_resource timed out".to_string())?
    .map_err(|e| format!("open_resource: {e}"))?;
    println!("LINK {}", session.link_id());
    session.set_config(transfer_config(Duration::from_secs(60)));
    match session.publish(&data).await {
        Ok(()) => println!("PUBLISH_OK {}", data.len()),
        Err(e) => println!("PUBLISH_ERR {:?} {e}", e.kind()),
    }
    // Let RNS finish its callback before the drop's link close reaches it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(session);
    Ok(())
}
