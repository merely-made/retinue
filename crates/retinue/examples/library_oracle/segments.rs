//! The big-transfer modes: a multi-segment Resource each way, and a request each way that
//! travels as a Resource.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::request::Request;

use crate::support::{
    accept_resource_link, hold_until_peer_closes, keep_announcing, report_received, resolve_rns,
    transfer_config,
};
use crate::{RNS_BIG_REQUEST_SEED, RNS_SINK_SEED};

/// Generous for a few MiB over loopback; RNS proves each segment before advertising the
/// next.
const SEGMENT_TIMEOUT: Duration = Duration::from_secs(240);

/// RNS publishes `data` to us, then we publish it to RNS.
pub(super) async fn resource_segments(
    endpoint: Arc<Endpoint>,
    data: Vec<u8>,
) -> Result<(), String> {
    let mut session = accept_resource_link(&endpoint).await?;
    session.set_config(transfer_config(SEGMENT_TIMEOUT));
    report_received(session.receive().await, &data);
    hold_until_peer_closes(&mut session).await;
    drop(session);

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
    session.set_config(transfer_config(SEGMENT_TIMEOUT));
    match session.publish(&data).await {
        Ok(()) => println!("PUBLISH_OK {}", data.len()),
        Err(e) => println!("PUBLISH_ERR {:?} {e}", e.kind()),
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// One request of `data` each way, too large for a packet, each answered with its echo.
pub(super) async fn request_resource(endpoint: Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["bigreq-retinue"]);
    endpoint.register_resource(name.clone(), b"bigreq");
    let announcing = keep_announcing(&endpoint, name, b"bigreq");
    let rns_name = DestinationName::new("retinue", ["bigreq-rns"]);
    let (dest, identity) = resolve_rns(&endpoint, &rns_name, &RNS_BIG_REQUEST_SEED)
        .await
        .ok_or("RNS request announce not seen")?;
    let mut outbound = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_resource(dest, identity),
    )
    .await
    .map_err(|_| "open_resource timed out".to_string())?
    .map_err(|e| format!("open_resource: {e}"))?;
    let accepted = tokio::time::timeout(Duration::from_secs(60), endpoint.accept_resource())
        .await
        .map_err(|_| "no inbound link".to_string())?
        .map_err(|e| format!("accept_resource: {e}"))?;
    announcing.abort();
    let mut inbound = accepted.session;
    inbound.set_config(transfer_config(Duration::from_secs(60)));
    outbound.set_config(transfer_config(Duration::from_secs(60)));

    let serve = async {
        match inbound.receive_raw_request().await {
            Ok(request) => {
                println!("IN_REQUEST {} {}", request.packed.len(), request.request_id);
                let data = Request::unpack(&request.packed)
                    .map(|request| request.data)
                    .unwrap_or_default();
                match inbound.respond_auto(request.request_id, data).await {
                    Ok(mode) => println!("IN_RESPOND {mode:?}"),
                    Err(e) => println!("IN_RESPOND_ERR {:?} {e}", e.kind()),
                }
            }
            Err(e) => println!("IN_REQUEST_ERR {:?} {e}", e.kind()),
        }
    };
    let ask = async {
        let request = Request::new(b"/echo", data.clone(), 1.0e9);
        println!(
            "OUT_REQUEST_ID {}",
            retinue::hash::AddressHash::of(&request.pack())
        );
        match outbound.request(&request).await {
            Ok(response) if response.data == data => {
                println!("OUT_REQUEST_OK {}", request.pack().len());
            }
            Ok(response) => println!("OUT_REQUEST_MISMATCH {}", response.data.len()),
            Err(e) => println!("OUT_REQUEST_ERR {:?} {e}", e.kind()),
        }
    };
    tokio::join!(serve, ask);
    // Let RNS read its response before the drops' link closes reach it.
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}
