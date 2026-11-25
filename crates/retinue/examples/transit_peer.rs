//! The transit link receipt's missing composition: a direct-PHY responder, and a requester
//! aimed at that responder rather than at the board (`design_docs/2026-08-14_transit_link_receipt_spec.md`).
//!
//! `link_peer`'s responder role over `node_link`'s radio seam. Each process drives one
//! direct-PHY modem; between them, possibly, a relay that is neither of them:
//!
//! ```text
//! requester ── modem ── RF ── (relay?) ── RF ── modem ── responder
//! ```
//!
//! Both sides send `len` random bytes generated at run time and log the sha256 of what they
//! sent and what they received, so a driver can match the two logs. The payload crosses as
//! one link packet (`form=packet`, refused when it cannot fit one) or as the endpoint's
//! best-effort stream as `link_peer` uses it (`form=stream`). The default is the ruled one,
//! 256 bytes over the stream (D13: "Yes, the stream"). The requester reports the hop
//! count and transport of each announce it heard for the responder, and the route it then
//! sends through. Nothing here steers the path: the endpoint picks from what it hears.
//!
//! ```text
//! transit_peer role=responder port=PORT [dtr=0|1] [len=N] [form=packet|stream]
//!              [window=SECONDS] [announce=SECONDS]
//! transit_peer role=requester port=PORT [dtr=0|1] [len=N] [form=packet|stream]
//!              [timeout=SECONDS] [settle=SECONDS]
//! ```
//!
//! Lines a driver reads: `ANNOUNCE`, `ROUTE`, `LINKED`, `ACCEPTED`, `SENT`, `RECEIVED`,
//! `RESULT`, `INBOUND`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, LinkStream};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tulle::PhyProfile;
use tulle::airtime::AirtimeBudget;
use tulle::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};

/// The responder's fixed test identity, so the requester knows its destination in advance
/// and learns only the identity and the route from the air.
const RESPONDER_SEED: [u8; 64] = [0x42; 64];
const REQUESTER_SEED: [u8; 64] = [0x24; 64];

/// The link MTU both sides offer: one direct-PHY frame (`selvage::MAX_RADIO_FRAME_LEN`).
const LINK_MTU: usize = 255;

/// Largest plaintext one link data packet carries at `mtu`, by the endpoint's own rule
/// (`Link.py` 512-514): what fits the token and minimal header with a one-byte IFAC
/// reserve, rounded to the cipher block, less one for padding.
const fn single_packet_max(mtu: usize) -> usize {
    let room =
        mtu.saturating_sub(1 + retinue::packet::HEADER_MIN_LEN + retinue::token::TOKEN_OVERHEAD);
    (room / 16 * 16).saturating_sub(1)
}

/// The board boot profile every Retinue V4 and T114 starts in, as in `node_link`.
fn board_boot_profile() -> PhyProfile {
    PhyProfile {
        frequency_hz: 906_875_000,
        bandwidth_hz: 250_000,
        spreading_factor: 11,
        coding_rate_denominator: 5,
        preamble_symbols: 16,
        sync_word: 0x2b,
        explicit_header: true,
        crc: true,
        invert_iq: false,
        tx_power_dbm: 17,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Packet,
    Stream,
}

struct Args(BTreeMap<String, String>);

impl Args {
    fn parse() -> Result<Self, String> {
        let mut map = BTreeMap::new();
        for arg in std::env::args().skip(1) {
            let (key, value) = arg
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got {arg:?}"))?;
            map.insert(key.to_string(), value.to_string());
        }
        Ok(Self(map))
    }

    fn text(&self, key: &str) -> Result<String, String> {
        self.0
            .get(key)
            .cloned()
            .ok_or_else(|| format!("missing {key}="))
    }

    fn number<T: std::str::FromStr>(&self, key: &str, default: T) -> Result<T, String> {
        match self.0.get(key) {
            None => Ok(default),
            Some(value) => value.parse().map_err(|_| format!("bad {key}={value}")),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn random_payload(len: usize) -> Result<Vec<u8>, String> {
    let mut payload = vec![0_u8; len];
    getrandom::fill(&mut payload).map_err(|e| format!("no entropy: {e}"))?;
    Ok(payload)
}

fn responder_name() -> DestinationName {
    DestinationName::new("retinue", ["transit"])
}

fn stamp(started: Instant) -> String {
    format!("t={:.1}", started.elapsed().as_secs_f64())
}

/// Send `payload` the chosen way. One `write_all` of at most one packet's plaintext leaves
/// as a single link data packet; a stream write may leave as several.
async fn send(stream: &mut LinkStream, payload: &[u8]) -> Result<(), String> {
    stream
        .write_all(payload)
        .await
        .map_err(|e| format!("write failed: {e}"))?;
    stream
        .flush()
        .await
        .map_err(|e| format!("flush failed: {e}"))
}

async fn receive(
    stream: &mut LinkStream,
    len: usize,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut received = vec![0_u8; len];
    tokio::time::timeout(timeout, stream.read_exact(&mut received))
        .await
        .map_err(|_| format!("receive timed out waiting for {len} bytes"))?
        .map_err(|e| format!("receive failed: {e}"))?;
    Ok(received)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let role = args.text("role")?;
    let port = args.text("port")?;
    let dtr = args.number("dtr", 1_u8)? != 0;
    let len: usize = args.number("len", 256)?;
    let form = match args
        .text("form")
        .unwrap_or_else(|_| "stream".into())
        .as_str()
    {
        "packet" => Form::Packet,
        "stream" => Form::Stream,
        other => return Err(format!("form={other} is not packet or stream").into()),
    };
    let max = single_packet_max(LINK_MTU);
    if len == 0 {
        return Err("len must be at least 1".into());
    }
    if form == Form::Packet && len > max {
        return Err(format!(
            "len={len} does not fit one link packet at MTU {LINK_MTU} (at most {max} bytes); use form=stream"
        )
        .into());
    }

    let mut radio = DirectPhySerialLink::open(
        &port,
        board_boot_profile(),
        AirtimeBudget::new(60_000, 60_000),
        DirectPhySerialConfig {
            dtr,
            online_timeout: Duration::from_secs(10),
            transmit_timeout: Duration::from_secs(10),
            ..DirectPhySerialConfig::default()
        },
    )?;
    tokio::time::timeout(Duration::from_secs(15), radio.wait_online()).await??;
    println!("MODEM online role={role} len={len} form={form:?} single_packet_max={max}");

    let seed = if role == "responder" {
        RESPONDER_SEED
    } else {
        REQUESTER_SEED
    };
    let endpoint = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&seed),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    endpoint.set_link_mtu(LINK_MTU as u32);
    let interface = endpoint.attach_interface();
    let driver = tokio::spawn(drive(interface, radio));

    let outcome = match role.as_str() {
        "responder" => {
            let window = Duration::from_secs(args.number("window", 180_u64)?);
            let every = Duration::from_secs(args.number("announce", 20_u64)?);
            responder(&endpoint, len, window, every).await
        }
        "requester" => {
            let timeout = Duration::from_secs(args.number("timeout", 120_u64)?);
            let settle = Duration::from_secs(args.number("settle", 20_u64)?);
            requester(&endpoint, len, timeout, settle).await
        }
        other => Err(format!("role={other} is not responder or requester")),
    };
    driver.abort();
    match outcome {
        Ok(()) => println!("RESULT PASS role={role}"),
        Err(reason) => println!("RESULT FAIL role={role} {reason}"),
    }
    Ok(())
}

/// Register, announce on a clock for the whole window, and answer every inbound link with
/// one exchange. Every accepted link is logged, so a control can show none arrived.
async fn responder(
    endpoint: &Endpoint,
    len: usize,
    window: Duration,
    every: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    let name = responder_name();
    let identity = PrivateIdentity::from_secret_bytes(&RESPONDER_SEED);
    println!(
        "RESPONDER_DEST {}",
        name.destination_hash(identity.public())
    );
    // `register` announces; the timed ones follow an interval later, not on top of it.
    endpoint.register(name.clone(), b"transit");
    println!("ANNOUNCED {}", stamp(started));
    let deadline = tokio::time::Instant::now() + window;
    let mut next_announce = tokio::time::Instant::now() + every;
    let mut inbound = 0_u32;
    let mut exchanged = 0_u32;
    while tokio::time::Instant::now() < deadline {
        if tokio::time::Instant::now() >= next_announce {
            endpoint.announce(&name, b"transit");
            println!("ANNOUNCED {}", stamp(started));
            next_announce = tokio::time::Instant::now() + every;
        }
        let wait = next_announce.min(deadline) - tokio::time::Instant::now();
        let Ok(accepted) = tokio::time::timeout(wait, endpoint.accept()).await else {
            continue;
        };
        let mut stream = accepted.map_err(|e| format!("accept failed: {e}"))?;
        inbound += 1;
        println!(
            "ACCEPTED link={} iface={:?} {}",
            stream.link_id(),
            stream.interface(),
            stamp(started)
        );
        match receive(&mut stream, len, Duration::from_secs(90)).await {
            Ok(received) => {
                println!(
                    "RECEIVED len={} sha256={} {}",
                    received.len(),
                    sha256_hex(&received),
                    stamp(started)
                );
                let reply = random_payload(len)?;
                send(&mut stream, &reply).await?;
                println!(
                    "SENT len={} sha256={} {}",
                    reply.len(),
                    sha256_hex(&reply),
                    stamp(started)
                );
                exchanged += 1;
                // Hold the link long enough for the reply to leave before the stream drops.
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
            Err(reason) => println!("EXCHANGE_FAILED {reason} {}", stamp(started)),
        }
    }
    println!("INBOUND accepted={inbound} exchanged={exchanged}");
    Ok(())
}

/// Wait for the responder's announce, listen a while longer for other copies of it, report
/// the route the endpoint chose, then link and exchange.
async fn requester(
    endpoint: &Endpoint,
    len: usize,
    timeout: Duration,
    settle: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    let identity = PrivateIdentity::from_secret_bytes(&RESPONDER_SEED);
    let dest: AddressHash = responder_name().destination_hash(identity.public());
    println!("REQUESTER_TARGET {dest}");
    let deadline = tokio::time::Instant::now() + timeout;
    let mut heard = None;
    let mut settle_until = None;
    loop {
        let now = tokio::time::Instant::now();
        let until = settle_until.unwrap_or(deadline).min(deadline);
        if now >= until {
            break;
        }
        let Ok(next) = tokio::time::timeout(until - now, endpoint.next_announcement()).await else {
            break;
        };
        let announce = next.map_err(|e| format!("announce stream failed: {e}"))?;
        if announce.destination != dest {
            println!(
                "ANNOUNCE other={} hops={}",
                announce.destination, announce.hops
            );
            continue;
        }
        println!(
            "ANNOUNCE responder hops={} transport={} {}",
            announce.hops,
            announce
                .transport
                .map(|t| t.to_string())
                .unwrap_or_else(|| "none".into()),
            stamp(started)
        );
        if heard.is_none() {
            heard = Some(announce.identity);
            settle_until = Some(tokio::time::Instant::now() + settle);
        }
    }
    let peer = heard.ok_or("no announce from the responder heard")?;
    let route = endpoint
        .route_facts_at(Instant::now())
        .into_iter()
        .find(|route| route.destination == dest);
    match route {
        Some(route) => println!(
            "ROUTE hops={} next_hop={} iface={:?}",
            route.hops,
            route
                .transport
                .map(|t| t.to_string())
                .unwrap_or_else(|| "direct".into()),
            route.interface
        ),
        None => println!("ROUTE none"),
    }

    let opened = Instant::now();
    let mut stream = tokio::time::timeout(timeout, endpoint.open(dest, peer))
        .await
        .map_err(|_| "link open timed out: no proof")?
        .map_err(|e| format!("link open failed: {e}"))?;
    println!(
        "LINKED link={} in {:.1}s {}",
        stream.link_id(),
        opened.elapsed().as_secs_f64(),
        stamp(started)
    );
    let payload = random_payload(len)?;
    send(&mut stream, &payload).await?;
    println!(
        "SENT len={} sha256={} {}",
        payload.len(),
        sha256_hex(&payload),
        stamp(started)
    );
    let received = receive(&mut stream, len, timeout).await?;
    println!(
        "RECEIVED len={} sha256={} {}",
        received.len(),
        sha256_hex(&received),
        stamp(started)
    );
    Ok(())
}
