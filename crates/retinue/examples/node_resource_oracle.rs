//! The Retinue half of the Node resource-metadata live gate
//! (`oracle/interop_node_resource_metadata.py`).
//!
//! A [`Node`] behind a minimal TCP shell announces `retinue.node-resource` until stock RNS
//! links to it, then receives RNS's `Resource(data, metadata=...)`. Each delivery prints
//! `RESOURCE <len> <sha256>` and `METADATA <hex>` (or `METADATA_NONE`).

use std::time::Duration;

use retinue::announce::AnnounceBlob;
use retinue::destination::DestinationName;
use retinue::identity::PrivateIdentity;
use retinue::iface::hdlc::{Deframer, frame};
use retinue::node::{Action, Node};
use retinue::packet::Packet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

const IDENTITY_SEED: [u8; 64] = [0x4E; 64];
const IFACE: u32 = 0;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh announce blob: a nonce, then the emission time in seconds, as RNS mints them.
fn blob(nonce: u8) -> AnnounceBlob {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("host clock is after the epoch")
        .as_secs();
    let mut blob = [nonce; 10];
    blob[5..].copy_from_slice(&seconds.to_be_bytes()[3..]);
    AnnounceBlob::from_wire(blob)
}

async fn run(listener: TcpListener) -> Result<(), Box<dyn std::error::Error>> {
    let mut node = Node::<8, 8, 2, 4>::new(
        PrivateIdentity::from_secret_bytes(&IDENTITY_SEED),
        DestinationName::new("retinue", ["node-resource"]).name_hash(),
    );
    let (stream, _) = listener.accept().await?;
    println!("CONNECTED");
    let (mut read, mut write) = stream.into_split();
    let (inbound_tx, mut inbound) = mpsc::channel::<Packet>(64);
    tokio::spawn(async move {
        let mut deframer = Deframer::new();
        let mut buf = vec![0_u8; 8192];
        while let Ok(n @ 1..) = read.read(&mut buf).await {
            for raw in deframer.push(&buf[..n]) {
                if let Ok(packet) = Packet::decode(&raw)
                    && inbound_tx.send(packet).await.is_err()
                {
                    return;
                }
            }
        }
    });

    let started = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let (mut linked, mut nonce) = (false, 0_u8);
    loop {
        let now = started.elapsed().as_millis() as u64;
        let actions = tokio::select! {
            packet = inbound.recv() => match packet {
                Some(packet) => node.ingest(IFACE, &packet, now),
                None => return Ok(()),
            },
            _ = tick.tick() => {
                if !linked {
                    nonce = nonce.wrapping_add(1);
                    let announce = node.announce(&blob(nonce), None);
                    write.write_all(&frame(&announce.encode())).await?;
                }
                node.poll(now, IFACE, None)
            }
        };
        for action in actions {
            match action {
                Action::Send { packet, .. } => write.write_all(&frame(&packet.encode())).await?,
                Action::LinkUp { .. } => {
                    linked = true;
                    println!("LINK_UP");
                }
                Action::Resource { data, metadata, .. } => {
                    println!(
                        "RESOURCE {} {}",
                        data.len(),
                        hex(&retinue::hash::full_hash(&data))
                    );
                    match metadata {
                        Some(metadata) => println!("METADATA {}", hex(&metadata)),
                        None => println!("METADATA_NONE"),
                    }
                }
                _ => {}
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seconds: u64 = std::env::args()
        .nth(1)
        .ok_or("usage: node_resource_oracle SECONDS")?
        .parse()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    println!("LISTENING {}", listener.local_addr()?.port());
    if let Ok(Err(error)) = tokio::time::timeout(Duration::from_secs(seconds), run(listener)).await
    {
        println!("MODE_ERR {error}");
    }
    println!("DONE");
    Ok(())
}
