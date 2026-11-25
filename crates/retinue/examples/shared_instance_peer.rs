//! The Retinue half of `oracle/interop_shared_instance.py`: attach to a stock shared instance
//! (rnsd) as a local client, announce one destination once, and echo inbound links.
//!
//! Environment: `RETINUE_SHARED_PORT`, the instance's TCP port.
//!
//! Prints `UP <transport id>`, `DEST <destination>` and `IFACE <id>`, then
//! `ANNOUNCE <dest> <iface> <hops>` for each announce heard. Commands on stdin, one per line:
//! `route <dest>` (answered `ROUTE <dest> <iface> <hops> <transport id or ->`), `link <dest>` (open a link and send `ping` on it), `online`.

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, InterfaceId, LinkStream, SharedInstance, SharedInstanceKind};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;

type Error = Box<dyn std::error::Error>;

fn parse_dest(text: &str) -> Option<AddressHash> {
    let bytes: [u8; 16] = hex::decode(text).ok()?.try_into().ok()?;
    Some(AddressHash::from_bytes(bytes))
}

async fn link(ep: Arc<Endpoint>, dest: AddressHash, held: Arc<Mutex<Vec<LinkStream>>>) {
    let Some(identity) = ep.resolve(dest) else {
        println!("LINK_FAILED unknown");
        return;
    };
    match tokio::time::timeout(Duration::from_secs(15), ep.open(dest, identity)).await {
        Ok(Ok(mut stream)) => {
            println!("LINKED {}", stream.link_id());
            let _ = stream.write_all(b"ping").await;
            let _ = stream.flush().await;
            // Keep it open, so the stock side sees the link stay up.
            held.lock().await.push(stream);
        }
        Ok(Err(e)) => println!("LINK_FAILED {e}"),
        Err(_) => println!("LINK_FAILED timeout"),
    }
}

fn command(ep: &Arc<Endpoint>, iface: InterfaceId, line: &str, held: &Arc<Mutex<Vec<LinkStream>>>) {
    let mut words = line.split_whitespace();
    match (words.next(), words.next().and_then(parse_dest)) {
        (Some("route"), Some(dest)) => {
            let facts = ep.route_facts_at(std::time::Instant::now());
            match facts.iter().find(|f| f.destination == dest) {
                Some(f) => {
                    let via = f.transport.map_or_else(|| "-".into(), |t| t.to_string());
                    println!("ROUTE {dest} {} {} {via}", f.interface, f.hops);
                }
                None => println!("ROUTE {dest} none"),
            }
        }
        (Some("link"), Some(dest)) => {
            tokio::spawn(link(Arc::clone(ep), dest, Arc::clone(held)));
        }
        (Some("online"), _) => println!("ONLINE {}", ep.interface_online(iface)),
        _ => println!("UNKNOWN {line}"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let port: u16 = std::env::var("RETINUE_SHARED_PORT")?.parse()?;
    let identity = PrivateIdentity::from_secret_bytes(&[0x5A; 64]);
    let ep = Arc::new(Endpoint::with_network_stamp_policy(
        identity.clone(),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let instance = SharedInstance {
        port,
        kind: SharedInstanceKind::Tcp,
        ..SharedInstance::default()
    };
    let iface = ep.attach_shared_instance(instance).await?;
    let name = DestinationName::new("retinue", ["shared"]);
    ep.register(name.clone(), b"shared");
    println!("UP {}", ep.identity().hash());
    println!("DEST {}", name.destination_hash(identity.public()));
    println!("IFACE {iface}");
    ep.announce(&name, b"shared");

    let announces = Arc::clone(&ep);
    tokio::spawn(async move {
        while let Ok(a) = announces.next_announcement().await {
            println!("ANNOUNCE {} {} {}", a.destination, a.interface, a.hops);
        }
    });
    let echo = Arc::clone(&ep);
    tokio::spawn(async move {
        while let Ok(mut stream) = echo.accept().await {
            println!("ACCEPTED {}", stream.link_id());
            tokio::spawn(async move {
                let mut buf = [0u8; 512];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    let held = Arc::new(Mutex::new(Vec::new()));
    let (tx, mut rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = rx.recv().await {
        command(&ep, iface, &line, &held);
    }
    Ok(())
}
