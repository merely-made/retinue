//! A KISS-framed TCP listener for `oracle/interop_tcp_kiss.py`, the server a stock
//! `TCPClientInterface` with `kiss_framing = yes` dials (`TCPInterface.py` 55-67, 322-388).
//!
//! Prints `LISTENING <port>`, `DEST <hash>`, `LEARNED <hash>` per announce heard, `LINK <id>`
//! and `DATA <len>` per proved link packet. Announces `retinue.kiss-peer` every 3 s.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ListenPolicy, ReceivedPayload, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ep = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x4b; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let policy = ListenPolicy {
        kiss_framing: true,
        ..ListenPolicy::default()
    };
    let listener = ep.listen_tcp_with("127.0.0.1:0".parse()?, policy).await?;
    println!("LISTENING {}", listener.local_addr().port());
    let name = DestinationName::new("retinue", ["kiss-peer"]);
    ep.register_resource(name.clone(), b"kiss-peer");
    println!("DEST {}", name.destination_hash(ep.identity()));
    let announcer = Arc::clone(&ep);
    tokio::spawn(async move {
        loop {
            announcer.announce(&name, b"kiss-peer");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
    let heard = Arc::clone(&ep);
    tokio::spawn(async move {
        while let Ok(announce) = heard.next_announcement().await {
            println!("LEARNED {}", announce.destination);
        }
    });
    while let Ok(accepted) = ep.accept_resource().await {
        let mut session = accepted.session;
        session.set_config(ResourceTransferConfig {
            timeout: Duration::from_secs(3600),
            ..ResourceTransferConfig::default()
        });
        println!("LINK {}", session.link_id());
        tokio::spawn(async move {
            while let Ok(payload) = session.receive().await {
                if let ReceivedPayload::Data(data) = payload {
                    println!("DATA {}", data.len());
                    let _ = session.prove_data();
                }
            }
        });
    }
    Ok(())
}
