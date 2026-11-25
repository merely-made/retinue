//! Retinue half of `oracle/interop_rns_config.py`: attach the interfaces an RNS config file
//! describes, then announce `retinue.rns-config` until stock links to it.
//!
//! `rns_config_peer CONFIG`: prints `ATTACHED <name>` per enabled section, `DEST <hash>`,
//! `LINK <id>` once a link is up, and `HOLD_END` when stock closes it.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, RnsConfig};
use retinue::identity::PrivateIdentity;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: rns_config_peer CONFIG")?;
    let config = RnsConfig::parse(&std::fs::read_to_string(path)?)?;
    let endpoint = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x5c; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    // Hold listeners and UDP handles for the life of the run.
    let mut attached = Vec::new();
    for section in config.interfaces {
        let section = section?;
        if section.enabled {
            attached.push(endpoint.attach_rns_interface(&section).await?);
            println!("ATTACHED {}", section.name);
        }
    }

    let name = DestinationName::new("retinue", ["rns-config"]);
    endpoint.register_resource(name.clone(), b"rns-config");
    println!("DEST {}", name.destination_hash(endpoint.identity()));
    let announcer = Arc::clone(&endpoint);
    let announcing = tokio::spawn(async move {
        loop {
            announcer.announce(&name, b"rns-config");
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    });
    let accepted = tokio::time::timeout(Duration::from_secs(60), endpoint.accept_resource()).await;
    announcing.abort();
    let mut session = accepted.map_err(|_| "no link")??.session;
    println!("LINK {}", session.link_id());
    let _ = session.receive().await;
    println!("HOLD_END");
    drop(attached);
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.shutdown(Duration::from_secs(3)),
    )
    .await;
    println!("DONE");
    Ok(())
}
