//! The Retinue half of the path-request live gate (`oracle/interop_path_request.py`).
//!
//! Stock RNS connects twice, as interfaces `A` and `B`, and calls `Transport.request_path`
//! for a destination Retinue owns but has not announced to it. Retinue must answer each
//! request once, on the interface the request came in on, so RNS learns the path from the
//! response alone.
//!
//! - `endpoint SECONDS`: an [`Endpoint`] owns the destination. Each RNS connection reaches
//!   it through a byte-for-byte relay that reports the path responses it carries.
//! - `node SECONDS`: a [`Node`] owns the destination, behind a minimal TCP shell that feeds
//!   it packets and carries out its sends. Its boot announce goes out on an interface RNS is
//!   not on, so it holds a blob to answer with while RNS has heard nothing.
//!
//! Each path response is printed as `PATH_RESPONSE A` or `PATH_RESPONSE B` as it leaves.

use std::net::SocketAddr;
use std::time::Duration;

use retinue::announce::AnnounceBlob;
use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::iface::hdlc::{Deframer, frame};
use retinue::node::{Action, Node};
use retinue::packet::{Packet, PacketType};
use retinue::path::CTX_PATH_RESPONSE;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const IDENTITY_SEED: [u8; 64] = [0x48; 64];
const LABELS: [&str; 2] = ["A", "B"];

fn is_path_response(raw: &[u8]) -> bool {
    Packet::decode(raw).is_ok_and(|packet| {
        packet.packet_type == PacketType::Announce && packet.context == CTX_PATH_RESPONSE
    })
}

/// Copy one direction verbatim, reporting each path response it carries toward RNS.
async fn relay(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, to_rns: Option<&'static str>) {
    let mut deframer = Deframer::new();
    let mut buf = vec![0_u8; 8192];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if to.write_all(&buf[..n]).await.is_err() {
            break;
        }
        if let Some(label) = to_rns {
            for raw in deframer.push(&buf[..n]) {
                if is_path_response(&raw) {
                    println!("PATH_RESPONSE {label}");
                }
            }
        }
    }
    let _ = to.shutdown().await;
}

async fn run_endpoint(listeners: Vec<TcpListener>) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&IDENTITY_SEED));
    // Registered with no interface attached, so its announce reaches nobody.
    endpoint.register(DestinationName::new("retinue", ["pathgate"]), b"endpoint");
    let inner: SocketAddr = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    for (listener, label) in listeners.into_iter().zip(LABELS) {
        let (outer, _) = listener.accept().await?;
        let (outer_read, outer_write) = outer.into_split();
        let (inner_read, inner_write) = TcpStream::connect(inner).await?.into_split();
        tokio::spawn(relay(outer_read, inner_write, None));
        tokio::spawn(relay(inner_read, outer_write, Some(label)));
        println!("CONNECTED {label}");
    }
    std::future::pending::<()>().await;
    Ok(())
}

async fn run_node(listeners: Vec<TcpListener>) -> Result<(), Box<dyn std::error::Error>> {
    let mut node = Node::<8, 8, 2, 4>::new(
        PrivateIdentity::from_secret_bytes(&IDENTITY_SEED),
        DestinationName::new("retinue", ["pathgate"]).name_hash(),
    )
    .with_app_data(b"node");
    const NOWHERE: u32 = 99;
    // Five nonce bytes, then the emission time in seconds, as RNS mints them.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let mut blob = [0x5A_u8; 10];
    blob[5..].copy_from_slice(&seconds.to_be_bytes()[3..]);
    let _ = node.poll(0, NOWHERE, Some(&AnnounceBlob::from_wire(blob)));

    let (inbound_tx, mut inbound) = mpsc::channel::<(u32, Packet)>(64);
    let mut writers = Vec::new();
    for (interface, listener) in (0_u32..).zip(listeners) {
        let (stream, _) = listener.accept().await?;
        let (mut read, write) = stream.into_split();
        writers.push(write);
        let tx = inbound_tx.clone();
        tokio::spawn(async move {
            let mut deframer = Deframer::new();
            let mut buf = vec![0_u8; 8192];
            while let Ok(n @ 1..) = read.read(&mut buf).await {
                for raw in deframer.push(&buf[..n]) {
                    if let Ok(packet) = Packet::decode(&raw)
                        && tx.send((interface, packet)).await.is_err()
                    {
                        return;
                    }
                }
            }
        });
        println!("CONNECTED {}", LABELS[interface as usize]);
    }

    let started = tokio::time::Instant::now();
    while let Some((interface, packet)) = inbound.recv().await {
        let now = started.elapsed().as_millis() as u64;
        for action in node.ingest(interface, &packet, now) {
            if let Action::Send { interface, packet } = action
                && let Some(writer) = writers.get_mut(interface as usize)
            {
                let wire = packet.encode();
                if is_path_response(&wire) {
                    println!("PATH_RESPONSE {}", LABELS[interface as usize]);
                }
                writer.write_all(&frame(&wire)).await?;
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, seconds] = args.as_slice() else {
        return Err("usage: path_request_oracle endpoint|node SECONDS".into());
    };
    let seconds: u64 = seconds.parse()?;
    let destination = DestinationName::new("retinue", ["pathgate"])
        .destination_hash(PrivateIdentity::from_secret_bytes(&IDENTITY_SEED).public());
    let a = TcpListener::bind("127.0.0.1:0").await?;
    let b = TcpListener::bind("127.0.0.1:0").await?;
    println!(
        "LISTENING {} {}",
        a.local_addr()?.port(),
        b.local_addr()?.port()
    );
    println!("DEST {destination}");
    let listeners = vec![a, b];
    let run = async {
        match mode.as_str() {
            "endpoint" => run_endpoint(listeners).await,
            "node" => run_node(listeners).await,
            other => Err(format!("unknown mode {other}").into()),
        }
    };
    if let Ok(Err(error)) = tokio::time::timeout(Duration::from_secs(seconds), run).await {
        println!("MODE_ERR {error}");
    }
    println!("DONE");
    Ok(())
}
