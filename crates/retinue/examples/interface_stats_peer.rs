//! Print per-interface statistics as JSON, for `oracle/interop_interface_stats.py`.
//!
//! Listens on 127.0.0.1 with IFAC from `RETINUE_IFAC=network_name,passphrase` and prints
//! `LISTENING <port>` and `UP <identity>`. Commands on stdin, one per line:
//! - `announce`: announce this peer's destination once, printing `ANNOUNCED <dest>`;
//! - `stats`: one `STATS <id> <json>` line per attached interface, then `END`.

use std::io::BufRead;

use retinue::Ifac;
use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spec = std::env::var("RETINUE_IFAC")?;
    let (network, passphrase) = spec.split_once(',').ok_or("RETINUE_IFAC=name,passphrase")?;
    let ep = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x3C; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let ifac = Ifac::for_stream(Some(network), Some(passphrase))?;
    let addr = ep
        .listen_tcp_with_ifac("127.0.0.1:0".parse()?, ifac)
        .await?;
    println!("LISTENING {}", addr.port());
    let name = DestinationName::new("retinue", ["interface_stats"]);
    ep.register(name.clone(), b"stats");
    println!("UP {}", ep.identity().hash());

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
            "announce" => {
                ep.announce(&name, b"stats");
                println!("ANNOUNCED {}", name.destination_hash(ep.identity()));
            }
            "stats" => {
                for id in ep.interface_ids() {
                    if let Some(counters) = ep.interface_counters(id) {
                        println!("STATS {id} {counters}");
                    }
                }
                println!("END");
            }
            other => println!("UNKNOWN {other}"),
        }
    }
    Ok(())
}
