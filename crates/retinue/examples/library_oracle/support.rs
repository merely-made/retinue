//! Announcing, resolving, and session helpers shared by the modes.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ReceivedPayload, ResourceSession, ResourceTransferConfig};
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::PrivateIdentity;

use crate::hex;

/// Announce `name` until the returned handle is aborted, so a late RNS start still hears one.
pub(super) fn keep_announcing(
    endpoint: &Arc<Endpoint>,
    name: DestinationName,
    app_data: &'static [u8],
) -> tokio::task::JoinHandle<()> {
    let endpoint = Arc::clone(endpoint);
    tokio::spawn(async move {
        loop {
            endpoint.announce(&name, app_data);
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    })
}

/// Wait until an announce for `name` under the identity from `seed` has been validated.
pub(super) async fn resolve_rns(
    endpoint: &Endpoint,
    name: &DestinationName,
    seed: &[u8; 64],
) -> Option<(AddressHash, retinue::identity::Identity)> {
    let dest = name.destination_hash(PrivateIdentity::from_secret_bytes(seed).public());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        if let Some(identity) = endpoint.resolve(dest) {
            return Some((dest, identity));
        }
        let _ =
            tokio::time::timeout(Duration::from_millis(500), endpoint.next_announcement()).await;
    }
    None
}

/// Wait for the endpoint to retire `link`. A reliable link is retired only after both
/// directions ended and everything this side sent was proven, so this is the
/// application-visible sign that the peer's proofs were accepted.
pub(super) async fn wait_link_retired(
    endpoint: &Endpoint,
    link: AddressHash,
    limit: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if !endpoint.link_facts().iter().any(|fact| fact.id == link) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

pub(super) fn transfer_config(timeout: Duration) -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout,
        ..ResourceTransferConfig::default()
    }
}

/// Accept the next resource link to `retinue.library-resource`, announcing until one
/// arrives.
pub(super) async fn accept_resource_link(
    endpoint: &Arc<Endpoint>,
) -> Result<ResourceSession, String> {
    let name = DestinationName::new("retinue", ["library-resource"]);
    endpoint.register_resource(name.clone(), b"library-resource");
    let announcing = keep_announcing(endpoint, name, b"library-resource");
    let accepted = tokio::time::timeout(Duration::from_secs(30), endpoint.accept_resource()).await;
    announcing.abort();
    let session = accepted
        .map_err(|_| "no resource link".to_string())?
        .map_err(|e| format!("accept_resource: {e}"))?
        .session;
    println!("LINK {}", session.link_id());
    Ok(session)
}

/// Report one received Resource against the expected bytes.
pub(super) fn report_received(received: std::io::Result<ReceivedPayload>, expected: &[u8]) {
    match received {
        Ok(ReceivedPayload::Resource(data)) => {
            println!("RESOURCE {} {}", data.len(), hex(&full_hash(&data)));
            if data == expected {
                println!("RESOURCE_OK");
            } else {
                println!("RESOURCE_MISMATCH");
            }
        }
        Ok(ReceivedPayload::Data(data)) => println!("UNEXPECTED_DATA {}", data.len()),
        Err(e) => println!("RECEIVE_ERR {:?} {e}", e.kind()),
    }
}

/// Keep the link (and with it the proof's chance to land, or be asked for again) until RNS
/// hangs up. Dropping the session sends a link close, which would end the RNS sender's
/// transfer for a reason that has nothing to do with the proof.
pub(super) async fn hold_until_peer_closes(session: &mut ResourceSession) {
    session.set_config(transfer_config(Duration::from_secs(40)));
    match session.receive().await {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => println!("PEER_CLOSED"),
        Err(e) => println!("HOLD_END {:?} {e}", e.kind()),
        Ok(_) => println!("HOLD_UNEXPECTED_PAYLOAD"),
    }
}
