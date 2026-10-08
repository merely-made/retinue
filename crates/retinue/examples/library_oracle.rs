//! The Retinue half of the library-path live oracle gates.
//!
//! Every mode drives the public [`Endpoint`] API, never the resource or channel state
//! machines directly, so a gate that passes here is evidence about what applications get.
//! The Python drivers in `oracle/` play stock RNS on the other end:
//!
//! - `resource-recv LEN SEED` (`interop_library_resource_recv.py`): RNS sends a Resource;
//!   [`Endpoint::accept_resource`] and [`ResourceSession::receive`] take it.
//! - `resource-send LEN SEED` (`interop_library_resource_send.py`): a [`ResourceSession`]
//!   opened with [`Endpoint::open_resource`] publishes to an RNS receiver.
//! - `stream-respond LEN SEED` (`interop_reliable_responder_send.py`): RNS opens a reliable
//!   link and reads what [`Endpoint::accept_reliable`]'s stream writes.
//! - `stream-open LEN SEED` (`interop_reliable_initiator_proofs.py`): an
//!   [`Endpoint::open_reliable`] stream reads what an RNS responder's Channel sends.
//!
//! The endpoint listens on a private port behind a byte-for-byte TCP relay. The relay only
//! copies; it also deframes a copy of each direction and tallies packets by type and
//! context, which is how a gate sees, for example, which packet type carried a resource
//! proof, or how many channel packets crossed the wire for a given number of messages.
//! Those tallies are printed as `TAP` lines when the mode ends, however it ends.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ReceivedPayload, ResourceSession, ResourceTransferConfig};
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::PrivateIdentity;
use retinue::iface::hdlc::Deframer;
use retinue::packet::{Packet, PacketType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const IDENTITY_SEED: [u8; 64] = [0x47; 64];
/// The RNS-side identity for the modes in which Retinue initiates. The Python drivers
/// build the same identity from the same 64 bytes.
const RNS_SINK_SEED: [u8; 64] = [0x5a; 64];
const RNS_STREAM_SEED: [u8; 64] = [0x5b; 64];

type Tally = Arc<Mutex<BTreeMap<(&'static str, &'static str, u8), u64>>>;

/// The payload both sides agree on: a xorshift32 stream, one low byte per step.
/// Pseudo-random, so neither bz2 nor anything else shrinks it.
fn payload(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.max(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn type_name(packet_type: PacketType) -> &'static str {
    match packet_type {
        PacketType::Data => "Data",
        PacketType::Announce => "Announce",
        PacketType::LinkRequest => "LinkRequest",
        PacketType::Proof => "Proof",
    }
}

/// Copy one direction verbatim, tallying a deframed copy of every packet.
async fn relay(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    direction: &'static str,
    tally: Tally,
) {
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
        for frame in deframer.push(&buf[..n]) {
            if let Ok(packet) = Packet::decode(&frame) {
                *tally
                    .lock()
                    .unwrap()
                    .entry((direction, type_name(packet.packet_type), packet.context))
                    .or_default() += 1;
            }
        }
    }
    let _ = to.shutdown().await;
}

/// Accept the RNS connection on `listener` and relay it to the endpoint at `inner`.
async fn tap(listener: TcpListener, inner: SocketAddr, tally: Tally) -> std::io::Result<()> {
    let (outer, _) = listener.accept().await?;
    let endpoint_side = TcpStream::connect(inner).await?;
    let (outer_read, outer_write) = outer.into_split();
    let (inner_read, inner_write) = endpoint_side.into_split();
    tokio::spawn(relay(outer_read, inner_write, "to_retinue", tally.clone()));
    tokio::spawn(relay(inner_read, outer_write, "to_rns", tally));
    Ok(())
}

fn print_tally(tally: &Tally) {
    for ((direction, packet_type, context), count) in tally.lock().unwrap().iter() {
        println!("TAP {direction} {packet_type} ctx=0x{context:02x} {count}");
    }
}

/// Announce `name` until the returned handle is aborted, so a late RNS start still hears one.
fn keep_announcing(
    endpoint: &Arc<Endpoint>,
    name: DestinationName,
    app_data: &'static [u8],
) -> tokio::task::JoinHandle<()> {
    let endpoint = Arc::clone(endpoint);
    tokio::spawn(async move {
        loop {
            endpoint.announce(&name, app_data);
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    })
}

/// Wait until an announce for `name` under the identity from `seed` has been validated.
async fn resolve_rns(
    endpoint: &Endpoint,
    name: &DestinationName,
    seed: &[u8; 64],
) -> Option<(AddressHash, retinue::identity::Identity)> {
    let dest = name.destination_hash(PrivateIdentity::from_secret_bytes(seed).public());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        if let Some(identity) = endpoint.resolve(dest) {
            return Some((dest, identity));
        }
        let _ =
            tokio::time::timeout(Duration::from_millis(500), endpoint.next_announcement()).await;
    }
    None
}

/// Wait for the endpoint to retire `link`. A reliable link is retired only after both
/// directions ended and everything this side sent was proven, so this is the
/// application-visible sign that the peer's proofs were accepted.
async fn wait_link_retired(endpoint: &Endpoint, link: AddressHash, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if !endpoint.link_facts().iter().any(|fact| fact.id == link) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn transfer_config(timeout: Duration) -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout,
        ..ResourceTransferConfig::default()
    }
}

async fn resource_recv(endpoint: Arc<Endpoint>, expected: Vec<u8>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["library-resource"]);
    endpoint.register_resource(name.clone(), b"library-resource");
    let announcing = keep_announcing(&endpoint, name, b"library-resource");
    let accepted = tokio::time::timeout(Duration::from_secs(30), endpoint.accept_resource()).await;
    announcing.abort();
    let mut session: ResourceSession = accepted
        .map_err(|_| "no resource link".to_string())?
        .map_err(|e| format!("accept_resource: {e}"))?
        .session;
    println!("LINK {}", session.link_id());

    session.set_config(transfer_config(Duration::from_secs(60)));
    match session.receive().await {
        Ok(ReceivedPayload::Resource(data)) => {
            println!("RESOURCE {} {}", data.len(), hex(&full_hash(&data)));
            if data == expected {
                println!("RESOURCE_OK");
            } else {
                println!("RESOURCE_MISMATCH");
            }
        }
        Ok(ReceivedPayload::Data(data)) => println!("UNEXPECTED_DATA {}", data.len()),
        Err(e) => println!("RECEIVE_ERR {:?} {e}", e.kind()),
    }

    // Keep the link (and with it the proof's chance to land) until RNS hangs up. Dropping
    // the session sends a link close, which would end the RNS sender's transfer for a
    // reason that has nothing to do with the proof.
    session.set_config(transfer_config(Duration::from_secs(40)));
    match session.receive().await {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => println!("PEER_CLOSED"),
        Err(e) => println!("HOLD_END {:?} {e}", e.kind()),
        Ok(_) => println!("HOLD_UNEXPECTED_PAYLOAD"),
    }
    Ok(())
}

async fn resource_send(endpoint: Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
    let name = DestinationName::new("retinue", ["library-sink"]);
    let (dest, identity) = resolve_rns(&endpoint, &name, &RNS_SINK_SEED)
        .await
        .ok_or("RNS sink announce not seen")?;
    println!("RESOLVED {dest}");
    let mut session = tokio::time::timeout(
        Duration::from_secs(20),
        endpoint.open_resource(dest, identity),
    )
    .await
    .map_err(|_| "open_resource timed out".to_string())?
    .map_err(|e| format!("open_resource: {e}"))?;
    println!("LINK {}", session.link_id());
    session.set_config(transfer_config(Duration::from_secs(60)));
    match session.publish(&data).await {
        Ok(()) => println!("PUBLISH_OK {}", data.len()),
        Err(e) => println!("PUBLISH_ERR {:?} {e}", e.kind()),
    }
    // Let RNS finish its callback before the drop's link close reaches it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(session);
    Ok(())
}

async fn stream_respond(endpoint: Arc<Endpoint>, data: Vec<u8>) -> Result<(), String> {
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

async fn stream_open(endpoint: Arc<Endpoint>, expected: Vec<u8>) -> Result<(), String> {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, len, seed] = args.as_slice() else {
        return Err("usage: library_oracle MODE LEN SEED".into());
    };
    let data = payload(len.parse()?, seed.parse()?);

    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &IDENTITY_SEED,
    )));
    let inner = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    println!("LISTENING {}", listener.local_addr()?.port());
    let tally: Tally = Arc::default();
    tap(listener, inner, tally.clone()).await?;
    // RNS's TCP client drops a peer whose first frame beats its own connection setup.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let run = match mode.as_str() {
        "resource-recv" => resource_recv(Arc::clone(&endpoint), data).await,
        "resource-send" => resource_send(Arc::clone(&endpoint), data).await,
        "stream-respond" => stream_respond(Arc::clone(&endpoint), data).await,
        "stream-open" => stream_open(Arc::clone(&endpoint), data).await,
        other => Err(format!("unknown mode {other}")),
    };
    if let Err(error) = &run {
        println!("MODE_ERR {error}");
    }
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.shutdown(Duration::from_secs(3)),
    )
    .await;
    // Give the relay a moment to see the last frames before reporting.
    tokio::time::sleep(Duration::from_millis(200)).await;
    print_tally(&tally);
    println!("DONE");
    run.map_err(Into::into)
}
