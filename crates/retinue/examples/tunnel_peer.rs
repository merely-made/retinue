//! The Retinue half of `oracle/interop_tunnel.py`: dial a stock hub with tunnel synthesis,
//! or listen as a transport for stock clients that synthesize; announce one destination
//! once and echo inbound links.
//!
//! Environment: `RETINUE_HUB=host:port` to dial, or `RETINUE_LISTEN=<port>` (0 picks one) to
//! listen as a transport.
//!
//! Prints `UP <transport id>`, `PUBKEY <transport public key>`, `DEST <destination>`, and
//! `LABEL <interface label>` when dialing or `LISTENING <port>` when listening. Commands on
//! stdin: `route <dest>`, `counters`.

use std::io::BufRead;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, TcpClient};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;

type Error = Box<dyn std::error::Error>;

fn parse_dest(text: &str) -> Option<AddressHash> {
    let bytes: [u8; 16] = hex::decode(text).ok()?.try_into().ok()?;
    Some(AddressHash::from_bytes(bytes))
}

fn command(ep: &Endpoint, line: &str) {
    let mut words = line.split_whitespace();
    match (words.next(), words.next().and_then(parse_dest)) {
        (Some("route"), Some(dest)) => match ep.route_to(dest) {
            Some((iface, hops)) => println!("ROUTE {dest} {iface} {hops}"),
            None => println!("ROUTE {dest} none"),
        },
        (Some("counters"), _) => {
            let r = ep.routing_counters();
            println!(
                "COUNTERS {} {}",
                r.tunnels_synthesized, r.tunnel_paths_restored
            );
        }
        _ => println!("UNKNOWN {line}"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let identity = PrivateIdentity::from_secret_bytes(&[0x7B; 64]);
    let ep = Arc::new(Endpoint::with_network_stamp_policy(
        identity.clone(),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    println!("UP {}", ep.identity().hash());
    println!(
        "PUBKEY {}",
        hex::encode(identity.public().to_public_bytes())
    );
    if let Ok(hub) = std::env::var("RETINUE_HUB") {
        let (host, port) = hub.rsplit_once(':').ok_or("RETINUE_HUB is host:port")?;
        let client = TcpClient::new(host, port.parse()?);
        // The label `TcpClient` synthesizes its tunnel under.
        println!("LABEL TCPClientInterface[{host}:{port}]");
        ep.attach_tcp(client).await?;
    } else {
        let port: u16 = std::env::var("RETINUE_LISTEN")?.parse()?;
        ep.enable_routing();
        let addr = ep.listen_tcp(([127, 0, 0, 1], port).into()).await?;
        println!("LISTENING {}", addr.port());
    }
    let name = DestinationName::new("retinue", ["tunnel"]);
    ep.register(name.clone(), b"tunnel");
    println!("DEST {}", name.destination_hash(identity.public()));
    ep.announce(&name, b"tunnel");

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

    let (tx, mut rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = rx.recv().await {
        command(&ep, &line);
    }
    Ok(())
}
