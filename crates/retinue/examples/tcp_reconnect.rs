//! A retinue TCP client that survives its hub restarting, for `oracle/interop_tcp_reconnect.py`.
//!
//! Dials RETINUE_HOST:RETINUE_PORT by name and prints, one per line:
//! `DEST <hash>`, `ATTACHED <id> <online>`, `ONLINE <id> <online>` on each change, and
//! `LEARNED <hash> <id>` per announce heard. It announces once on first coming online.
//! Commands on stdin: `ANNOUNCE`, and `LINK <hash>`, which prints `ROUTE <hash> <id|none>`
//! then `LINKED` or `LINK_FAILED`.

use std::time::Duration;

use tokio::sync::mpsc;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, TcpClient};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::var("RETINUE_HOST").unwrap_or_else(|_| "localhost".into());
    let port: u16 = std::env::var("RETINUE_PORT")?.parse()?;
    let identity = PrivateIdentity::from_secret_bytes(&[0x5c; 64]);
    let ep = std::sync::Arc::new(Endpoint::new(identity.clone()));
    let name = DestinationName::new("retinue", ["reconnect"]);
    ep.register(name.clone(), b"reconnect");
    println!("DEST {}", name.destination_hash(identity.public()));

    let id = ep.attach_tcp(TcpClient::new(host, port)).await?;
    let mut online = ep.interface_online(id);
    println!("ATTACHED {id} {online}");

    let watcher = std::sync::Arc::clone(&ep);
    let watched = name.clone();
    tokio::spawn(async move {
        let mut announced = false;
        loop {
            let now = watcher.interface_online(id);
            if now != online {
                online = now;
                println!("ONLINE {id} {online}");
            }
            if online && !announced {
                announced = true;
                watcher.announce(&watched, b"reconnect");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let listener = std::sync::Arc::clone(&ep);
    tokio::spawn(async move {
        while let Ok(heard) = listener.next_announcement().await {
            println!("LEARNED {} {}", heard.destination, heard.interface);
        }
    });

    let mut links = Vec::new();
    let mut lines = stdin_lines();
    while let Some(line) = lines.recv().await {
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("ANNOUNCE"), _) => {
                ep.announce(&name, b"reconnect");
                println!("ANNOUNCED");
            }
            (Some("LINK"), Some(hex)) => {
                let bytes: [u8; 16] = hex::decode(hex)?.try_into().map_err(|_| "bad hash")?;
                let dest = AddressHash::from_bytes(bytes);
                match ep.route_to(dest) {
                    Some((via, hops)) => println!("ROUTE {dest} {via} {hops}"),
                    None => println!("ROUTE {dest} none"),
                }
                let Some(peer) = ep.resolve(dest) else {
                    println!("LINK_FAILED unknown");
                    continue;
                };
                match tokio::time::timeout(Duration::from_secs(10), ep.open(dest, peer)).await {
                    Ok(Ok(stream)) => {
                        println!("LINKED {}", stream.link_id());
                        links.push(stream);
                    }
                    Ok(Err(e)) => println!("LINK_FAILED {e}"),
                    Err(_) => println!("LINK_FAILED timeout"),
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Stdin lines, read on a thread: the crate's tokio has no stdin.
fn stdin_lines() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}
