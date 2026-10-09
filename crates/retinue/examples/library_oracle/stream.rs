//! The reliable-stream modes: respond to an RNS initiator, and open to an RNS responder.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::RNS_STREAM_SEED;
use crate::support::{keep_announcing, resolve_rns, wait_link_retired};

pub(super) async fn stream_respond(endpoint: Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["reliable-proofs"]);
    endpoint.register_reliable(name.clone(), b"reliable-proofs");
    let announcing = keep_announcing(&endpoint, name, b"reliable-proofs");
    let accepted = tokio::time::timeout(Duration::from_secs(30), endpoint.accept_reliable()).await;
    announcing.abort();
    let mut link = accepted
        .map_err(|_| "no reliable link".to_string())?
        .map_err(|e| format!("accept_reliable: {e}"))?;
    let link_id = link.link_id();
    println!("LINK {link_id}");

    tokio::time::timeout(Duration::from_secs(40), async {
        link.write_all(&data).await?;
        link.shutdown().await
    })
    .await
    .map_err(|_| "write timed out".to_string())?
    .map_err(|e| format!("write: {e}"))?;
    println!("SENT_EOF {}", data.len());

    let mut received = Vec::new();
    match tokio::time::timeout(Duration::from_secs(40), link.read_to_end(&mut received)).await {
        Ok(Ok(_)) => println!("READ_EOF {}", received.len()),
        Ok(Err(e)) => println!("READ_ERR {:?} {e}", e.kind()),
        Err(_) => println!("READ_TIMEOUT {}", received.len()),
    }
    if wait_link_retired(&endpoint, link_id, Duration::from_secs(20)).await {
        println!("STREAM_IDLE");
    } else {
        println!("STREAM_NOT_IDLE");
    }
    Ok(())
}

pub(super) async fn stream_open(endpoint: Arc<Endpoint>, expected: Vec<u8>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["reliable-sink"]);
    let (dest, identity) = resolve_rns(&endpoint, &name, &RNS_STREAM_SEED)
        .await
        .ok_or("RNS stream announce not seen")?;
    println!("RESOLVED {dest}");
    let mut link = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_reliable(dest, identity),
    )
    .await
    .map_err(|_| "open_reliable timed out".to_string())?
    .map_err(|e| format!("open_reliable: {e}"))?;
    let link_id = link.link_id();
    println!("LINK {link_id}");

    let mut received = Vec::new();
    match tokio::time::timeout(Duration::from_secs(60), link.read_to_end(&mut received)).await {
        Ok(Ok(_)) => println!("READ_EOF {}", received.len()),
        Ok(Err(e)) => println!("READ_ERR {:?} {e}", e.kind()),
        Err(_) => println!("READ_TIMEOUT {}", received.len()),
    }
    if received == expected {
        println!("RECV_OK");
    } else {
        println!("RECV_MISMATCH {} of {}", received.len(), expected.len());
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), link.shutdown()).await;
    if wait_link_retired(&endpoint, link_id, Duration::from_secs(15)).await {
        println!("STREAM_IDLE");
    } else {
        println!("STREAM_NOT_IDLE");
    }
    Ok(())
}
