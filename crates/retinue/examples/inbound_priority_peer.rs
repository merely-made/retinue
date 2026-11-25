//! Echo link traffic while announces flood in, for `oracle/interop_inbound_priority.py`.
//!
//! Listens on 127.0.0.1 and serves `retinue.priority`: every link data packet is proved and
//! echoed. Prints `LISTENING <port>` and `UP <dest>`. Commands on stdin, one per line:
//! `announce` re-announces; `lanes` prints `LANES <data> <announce> <path_request>
//! <ingress_limited>`, the inbound lane drop counters, then `STATS <id> <json>` per interface.

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, SessionInbound};
use retinue::identity::PrivateIdentity;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ep = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x3B; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let addr = ep.listen_tcp("127.0.0.1:0".parse()?).await?;
    println!("LISTENING {}", addr.port());
    let name = DestinationName::new("retinue", ["priority"]);
    ep.register_resource(name.clone(), b"priority");
    println!("UP {}", name.destination_hash(ep.identity()));

    let server = Arc::clone(&ep);
    tokio::spawn(async move {
        while let Ok(accepted) = server.accept_resource().await {
            println!("LINK {}", accepted.interface);
            let mut session = accepted.session;
            tokio::spawn(async move {
                while let Ok(inbound) = session.next_inbound(Duration::from_secs(300)).await {
                    if let SessionInbound::Data(data) = inbound {
                        session.prove(&data);
                        session.send_data(&data.data);
                    }
                }
            });
        }
    });

    let (tx, mut rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = rx.recv().await {
        match line.trim() {
            "announce" => ep.announce(&name, b"priority"),
            "lanes" => {
                let c = ep.routing_counters();
                println!(
                    "LANES {} {} {} {}",
                    c.inbound_data_dropped,
                    c.inbound_announces_dropped,
                    c.inbound_path_requests_dropped,
                    c.inbound_ingress_limited_dropped
                );
                for id in ep.interface_ids() {
                    if let Some(counters) = ep.interface_counters(id) {
                        println!("STATS {id} {counters}");
                    }
                }
            }
            other => println!("UNKNOWN {other}"),
        }
    }
    Ok(())
}
