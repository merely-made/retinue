//! The liveness mode: idle resource links each way, then an MDU-sized request on each.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::request::Request;

use crate::support::{keep_announcing, resolve_rns, transfer_config};
use crate::{LINK_MDU, RNS_LIVENESS_SEED};

/// Hold an outbound and an inbound resource link idle for `hold`, then exchange a request
/// of exactly [`LINK_MDU`] bytes on each. The inbound request is RNS's; the outbound one
/// carries `data` padded to the MDU and expects it echoed.
pub(super) async fn liveness(
    endpoint: Arc<Endpoint>,
    hold: Duration,
    data: Vec<u8>,
) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["liveness-retinue"]);
    endpoint.register_resource(name.clone(), b"liveness");
    let announcing = keep_announcing(&endpoint, name, b"liveness");
    let rns_name = DestinationName::new("retinue", ["liveness-rns"]);
    let (dest, identity) = resolve_rns(&endpoint, &rns_name, &RNS_LIVENESS_SEED)
        .await
        .ok_or("RNS liveness announce not seen")?;
    let mut outbound = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_resource(dest, identity),
    )
    .await
    .map_err(|_| "open_resource timed out".to_string())?
    .map_err(|e| format!("open_resource: {e}"))?;
    println!("OUT_LINK {}", outbound.link_id());
    let accepted = tokio::time::timeout(Duration::from_secs(60), endpoint.accept_resource())
        .await
        .map_err(|_| "no inbound link".to_string())?
        .map_err(|e| format!("accept_resource: {e}"))?;
    announcing.abort();
    let mut inbound = accepted.session;
    println!("IN_LINK {}", inbound.link_id());
    inbound.set_config(transfer_config(hold + Duration::from_secs(60)));
    outbound.set_config(transfer_config(Duration::from_secs(30)));

    let serve = async {
        match inbound.receive_raw_request().await {
            Ok(request) => {
                println!("IN_REQUEST {}", request.packed.len());
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
        tokio::time::sleep(hold).await;
        println!("HELD {}", endpoint.link_facts().len());
        let request = (0..LINK_MDU)
            .map(|n| Request::new(b"/mdu", data[..n.min(data.len())].to_vec(), 1.0e9))
            .find(|request| request.pack().len() == LINK_MDU)
            .expect("a prefix packs to the MDU");
        let sent = request.data.clone();
        match outbound.request(&request).await {
            Ok(response) if response.data == sent => {
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
