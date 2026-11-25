//! retinue on an embedded emissary, driven over stdin/stdout by the oracle's
//! `interop_i2p.py` against stock RNS (I2P plan, I2P4).
//!
//! ```text
//! rns_i2p_peer run --storage <emissary dir> --carrier <tcp:HOST:PORT | i2p-dial:PEER | i2p-listen>
//!              [--ifac NETWORK:PASSPHRASE] [--seed HEXBYTE] [--sam HOST:PORT] [--keep-session]
//! rns_i2p_peer keyfile <rns storagepath> <interface name>
//! ```
//!
//! Commands on stdin: `ANNOUNCE`, `ECHO <dest> <msg>`, `PUBLISH <dest> <len>`,
//! `ROUTE <dest>`, `STATS`, `EXIT`. Events on stdout, one per line: `READY`, `B32`, `HEARD`, `ONLINE`,
//! `IFACES`, `RX`, `ECHO_OK`/`ECHO_ERR`, `PUBLISH_OK`/`PUBLISH_ERR`, `RES_IN`, `ROUTE`,
//! `SAM_CMD`/`SAM_REPLY` (from a logging proxy in front of emissary's SAM port, so redial
//! timing is observable without instrumenting retinue), and `ROUTER_END`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use murm_i2p::{EmbeddedRouter, RouterConfig, Transit};
use retinue::destination::DestinationName;
use retinue::endpoint::{
    Endpoint, HwMtu, I2pListen, I2pPeer, ResourceTransferConfig, TcpClient, i2p_keyfile,
};
use retinue::hash::AddressHash;
use retinue::identity::{Identity, PrivateIdentity};
use retinue::ifac::Ifac;
use truchman::{Bridge, PrivateKey};

type Heard = Arc<Mutex<HashMap<AddressHash, Identity>>>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("keyfile") if args.len() == 3 => keyfile(&args[1], &args[2]),
        Some("run") => tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(run(&args[1..])),
        _ => {
            eprintln!("usage: rns_i2p_peer run ... | keyfile <storagepath> <name>");
            2
        },
    };
    std::process::exit(code);
}

/// Print where retinue looks for interface `name`'s key under RNS `storage`, and the b32 of
/// the key found there.
fn keyfile(storage: &str, name: &str) -> i32 {
    let storage = std::path::Path::new(storage);
    let secret = std::fs::read(storage.join("transport_identity")).expect("transport_identity");
    let secret: [u8; 64] = secret.as_slice().try_into().expect("64-byte identity");
    let transport = PrivateIdentity::from_secret_bytes(&secret).hash();
    let path = i2p_keyfile(storage, name, transport);
    match std::fs::read_to_string(&path).map(|t| PrivateKey::from_base64(&t)) {
        Ok(Ok(key)) => {
            println!("KEYFILE {} {}", path.display(), key.destination().b32());
            0
        },
        _ => {
            println!("KEYFILE_MISSING {}", path.display());
            1
        },
    }
}

fn arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_dest(text: &str) -> Option<AddressHash> {
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    AddressHash::from_slice(&bytes)
}

/// The oracle's payload, byte for byte: `(i * 31 + 7) mod 256`.
fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
        .collect()
}

fn ms(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

async fn run(args: &[String]) -> i32 {
    let start = Instant::now();
    let carrier = arg(args, "--carrier").expect("--carrier");
    let seed = arg(args, "--seed").map_or(0x5a, |s| u8::from_str_radix(s, 16).expect("--seed"));
    let ifac = arg(args, "--ifac").map(|v| {
        let (net, pass) = v.split_once(':').expect("--ifac NETWORK:PASSPHRASE");
        Ifac::for_stream(Some(net), Some(pass)).expect("ifac")
    });

    let ep = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[seed; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let stream_name = DestinationName::new("i2p4", ["retinue"]);
    let resource_name = DestinationName::new("i2p4", ["retinue-res"]);
    ep.register(stream_name.clone(), b"stream");
    ep.register_resource(resource_name.clone(), b"resource");
    serve(&ep, start);

    // `--sam HOST:PORT` uses an external router's bridge instead of an embedded emissary.
    let external = arg(args, "--sam").map(|a| Bridge::new(a.parse().expect("--sam HOST:PORT")));
    let router = if carrier.starts_with("i2p") && external.is_none() {
        let storage = arg(args, "--storage").expect("--storage");
        let router = EmbeddedRouter::start(RouterConfig::new(storage, Transit::Off, false))
            .await
            .expect("emissary starts");
        println!("ROUTER {:?} {}", router.report(), ms(start));
        Some(router)
    } else {
        None
    };
    let bridge = match (&router, external) {
        (Some(router), _) => Some(logging_proxy(router.bridge(), start).await),
        (None, Some(external)) => Some(logging_proxy(external, start).await),
        (None, None) => None,
    };

    let mut watched = None;
    if let Some(addr) = carrier.strip_prefix("tcp:") {
        let (host, port) = addr.rsplit_once(':').expect("tcp:HOST:PORT");
        let client = TcpClient {
            ifac: ifac.clone(),
            hw_mtu: HwMtu::Protocol,
            ..TcpClient::new(host, port.parse().expect("port"))
        };
        watched = Some(ep.attach_tcp(client).await.expect("tcp attach"));
    } else if let Some(peer) = carrier.strip_prefix("i2p-dial:") {
        let mut dial = I2pPeer::new(bridge.unwrap(), "I2P", peer);
        dial.ifac = ifac.clone();
        if args.iter().any(|a| a == "--keep-session") {
            dial.fresh_session_per_redial = false;
        }
        watched = Some(ep.attach_i2p_peer(dial).expect("i2p attach"));
    } else if carrier == "i2p-listen" {
        let bridge = bridge.unwrap();
        let key = bridge
            .generate(truchman::SIGNATURE_ED25519)
            .await
            .expect("destination");
        let mut listen = I2pListen::new(bridge, key.clone());
        listen.ifac = ifac.clone();
        ep.listen_i2p(listen).expect("i2p listen");
        println!("B32 {}", key.destination().b32());
    } else {
        panic!("unknown carrier {carrier}");
    }
    watch_interfaces(&ep, watched, start);
    println!(
        "READY {} {} {}",
        hex(stream_name.destination_hash(ep.identity()).as_slice()),
        hex(resource_name.destination_hash(ep.identity()).as_slice()),
        ms(start)
    );

    let heard: Heard = Arc::default();
    listen_announces(&ep, Arc::clone(&heard), start);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let end = async {
        while let Ok(Some(line)) = lines.next_line().await {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["ANNOUNCE"] => {
                    ep.announce(&stream_name, b"stream");
                    ep.announce(&resource_name, b"resource");
                },
                ["ECHO", dest, msg] => echo(&ep, &heard, dest, msg, start).await,
                ["PUBLISH", dest, len] => publish(&ep, &heard, dest, len, start).await,
                ["ROUTE", dest] => {
                    let known = parse_dest(dest).and_then(|d| ep.route_to(d));
                    println!("ROUTE {dest} {} {}", known.is_some(), ms(start));
                },
                ["STATS"] => {
                    for id in ep.interface_ids() {
                        if let Some(c) = ep.interface_counters(id) {
                            println!(
                                "STATS {id:?} online={} rx={} tx={} rx_bytes={} tx_bytes={} {}",
                                ep.interface_online(id),
                                c.rx_packets,
                                c.tx_packets,
                                c.rx_bytes,
                                c.tx_bytes,
                                ms(start)
                            );
                        }
                    }
                },
                ["EXIT"] => return 0,
                _ => println!("BAD_COMMAND {line}"),
            }
        }
        0
    };
    let code = match &router {
        Some(router) => tokio::select! {
            code = end => code,
            how = router.stopped() => {
                println!("ROUTER_END {how:?} {}", ms(start));
                3
            }
        },
        None => end.await,
    };
    if let Some(router) = router {
        router.shutdown().await;
    }
    code
}

/// Echo stream data and receive Resources, printing what arrives.
fn serve(ep: &Arc<Endpoint>, start: Instant) {
    let streams = Arc::clone(ep);
    tokio::spawn(async move {
        while let Ok(mut stream) = streams.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 512];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    println!("RX {} {}", String::from_utf8_lossy(&buf[..n]), ms(start));
                    let mut reply = b"echo:".to_vec();
                    reply.extend_from_slice(&buf[..n]);
                    if stream.write_all(&reply).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let resources = Arc::clone(ep);
    tokio::spawn(async move {
        while let Ok(mut accepted) = resources.accept_resource().await {
            match accepted.session.receive().await {
                Ok(retinue::endpoint::ReceivedPayload::Resource(data)) => println!(
                    "RES_IN {} {} {}",
                    data.len(),
                    hex(&Sha256::digest(&data)),
                    ms(start)
                ),
                Ok(other) => println!("RES_OTHER {other:?}"),
                Err(e) => println!("RES_ERR {e}"),
            }
        }
    });
}

fn listen_announces(ep: &Arc<Endpoint>, heard: Heard, start: Instant) {
    let ep = Arc::clone(ep);
    tokio::spawn(async move {
        while let Ok(fact) = ep.next_announcement().await {
            heard
                .lock()
                .unwrap()
                .insert(fact.destination, fact.identity);
            println!(
                "HEARD {} {} {}",
                hex(fact.destination.as_slice()),
                String::from_utf8_lossy(&fact.app_data),
                ms(start)
            );
        }
    });
}

/// Report the dialed interface going online and offline, and the interface count (a
/// listener's spawned interfaces).
fn watch_interfaces(
    ep: &Arc<Endpoint>,
    watched: Option<retinue::endpoint::InterfaceId>,
    start: Instant,
) {
    let ep = Arc::clone(ep);
    tokio::spawn(async move {
        let (mut online, mut count) = (None, None);
        loop {
            if let Some(id) = watched {
                let now = ep.interface_online(id);
                if online != Some(now) {
                    println!("ONLINE {now} {}", ms(start));
                    online = Some(now);
                }
            }
            let now = ep.interface_count();
            if count != Some(now) {
                println!("IFACES {now} {}", ms(start));
                count = Some(now);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
}

async fn echo(ep: &Endpoint, heard: &Heard, dest: &str, msg: &str, start: Instant) {
    let Some((dest, identity)) =
        parse_dest(dest).and_then(|d| Some((d, *heard.lock().unwrap().get(&d)?)))
    else {
        println!("ECHO_ERR unknown destination");
        return;
    };
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        let mut stream = ep.open(dest, identity).await?;
        stream.write_all(msg.as_bytes()).await?;
        stream.flush().await?;
        let mut buf = [0u8; 512];
        let n = stream.read(&mut buf).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf[..n]).into_owned())
    })
    .await;
    match result {
        Ok(Ok(reply)) => println!("ECHO_OK {reply} {}", ms(start)),
        Ok(Err(e)) => println!("ECHO_ERR {e}"),
        Err(_) => println!("ECHO_ERR timeout"),
    }
}

async fn publish(ep: &Endpoint, heard: &Heard, dest: &str, len: &str, start: Instant) {
    let Some((dest, identity)) =
        parse_dest(dest).and_then(|d| Some((d, *heard.lock().unwrap().get(&d)?)))
    else {
        println!("PUBLISH_ERR unknown destination");
        return;
    };
    let data = payload(len.parse().unwrap_or(12_000));
    let config = ResourceTransferConfig {
        timeout: Duration::from_secs(60),
        retry_interval: Duration::from_secs(2),
        request_window: 1,
    };
    match ep
        .send_payload_with_config(dest, identity, &data, config)
        .await
    {
        Ok(mode) => println!(
            "PUBLISH_OK {} {mode:?} {} {}",
            data.len(),
            hex(&Sha256::digest(&data)),
            ms(start)
        ),
        Err(e) => println!("PUBLISH_ERR {e}"),
    }
}

/// A SAM proxy in front of `bridge` that prints each connection's command (the line after
/// `HELLO`) and the bridge's reply to it, then splices the two sockets.
async fn logging_proxy(bridge: Bridge, start: Instant) -> Bridge {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(upstream) = TcpStream::connect(bridge.addr()).await else {
                    return;
                };
                let _ = splice_logged(client, upstream, start).await;
            });
        }
    });
    Bridge::new(addr)
}

async fn splice_logged(
    mut client: TcpStream,
    mut upstream: TcpStream,
    start: Instant,
) -> std::io::Result<()> {
    // HELLO and its reply, then the command and its reply; then raw bytes either way.
    for i in 0..2 {
        let line = copy_line(&mut client, &mut upstream).await?;
        if i == 1 {
            println!("SAM_CMD {} {}", words(&line, 2), ms(start));
        }
        let reply = copy_line(&mut upstream, &mut client).await?;
        if i == 1 {
            println!("SAM_REPLY {} {}", words(&reply, 3), ms(start));
        }
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Copy one line, newline included, returning it without the newline. The line is forwarded
/// in a single write: i2pd 3.0.0 drops a SAM connection whose `HELLO` arrives split across
/// segments (found 2026-10-23; see the I2P plan's upstream ledger).
async fn copy_line(from: &mut TcpStream, to: &mut TcpStream) -> std::io::Result<String> {
    let mut line = Vec::new();
    loop {
        let byte = from.read_u8().await?;
        line.push(byte);
        if byte == b'\n' {
            to.write_all(&line).await?;
            line.pop();
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
    }
}

fn words(line: &str, n: usize) -> String {
    line.split_whitespace()
        .take(n)
        .collect::<Vec<_>>()
        .join(" ")
}
