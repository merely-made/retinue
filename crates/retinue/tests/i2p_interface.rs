//! The I2P interface over a fake loopback SAM bridge. The fake keeps sessions, answers name
//! lookups for the destinations it holds (inside a session only, as emissary does), and joins each `STREAM CONNECT` to a waiting
//! `STREAM ACCEPT` at that destination, as a router does across the network.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use retinue::destination::DestinationName;
use retinue::endpoint::{
    Carrier, Endpoint, I2pListen, I2pPeer, I2pTimers, RnsConfig, i2p_keyfile,
    load_or_generate_i2p_key,
};
use retinue::identity::PrivateIdentity;
use retinue::truchman::{self, Bridge, Key, PrivateKey, SessionOptions, StreamSession};

/// Destination and private-key base64 for a destination with a 4-byte key certificate.
fn keypair(seed: u8) -> (String, String) {
    let mut public: Vec<u8> = (0..384)
        .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
        .collect();
    public.extend([5, 0, 4, 0, 7, 0, 4]);
    let mut private = public.clone();
    private.extend([seed; 288]);
    (
        truchman::base64_encode(&public),
        truchman::base64_encode(&private),
    )
}

fn public_of(private: &str) -> String {
    PrivateKey::from_base64(private)
        .unwrap()
        .destination()
        .to_base64()
}

#[derive(Default)]
struct Router {
    /// Session ID to its destination (public base64).
    sessions: HashMap<String, String>,
    /// Destination to sockets waiting in `STREAM ACCEPT`.
    acceptors: HashMap<String, VecDeque<TcpStream>>,
    /// Every command line but `HELLO`.
    log: Vec<String>,
    next_seed: u8,
}

type Shared = Arc<Mutex<Router>>;

async fn fake_router() -> (Bridge, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bridge = Bridge::new(listener.local_addr().unwrap());
    let router: Shared = Arc::new(Mutex::new(Router {
        next_seed: 100,
        ..Router::default()
    }));
    let r = Arc::clone(&router);
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(serve(tcp, Arc::clone(&r)));
        }
    });
    (bridge, router)
}

/// One line, read a byte at a time so nothing after it is consumed.
async fn line(tcp: &mut TcpStream) -> Option<String> {
    let mut out = Vec::new();
    loop {
        let byte = tcp.read_u8().await.ok()?;
        if byte == b'\n' {
            return String::from_utf8(out).ok();
        }
        out.push(byte);
    }
}

fn pair<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(' ')
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

/// The destination whose b32 name is `name`, among the sessions the router holds.
fn lookup(router: &Shared, name: &str) -> Option<String> {
    router.lock().unwrap().sessions.values().find_map(|dest| {
        let d = truchman::Destination::from_base64(dest).unwrap();
        (d.b32() == name).then(|| dest.clone())
    })
}

async fn serve(mut tcp: TcpStream, router: Shared) {
    while let Some(cmd) = line(&mut tcp).await {
        if cmd.starts_with("HELLO") {
            tcp.write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
                .await
                .unwrap();
            continue;
        }
        router.lock().unwrap().log.push(cmd.clone());
        if cmd.starts_with("DEST GENERATE") {
            let seed = {
                let mut r = router.lock().unwrap();
                r.next_seed += 1;
                r.next_seed
            };
            let (public, private) = keypair(seed);
            let reply = format!("DEST REPLY PUB={public} PRIV={private}\n");
            tcp.write_all(reply.as_bytes()).await.unwrap();
        } else if cmd.starts_with("NAMING LOOKUP") {
            // As emissary does: outside a session no name resolves.
            let name = pair(&cmd, "NAME").unwrap().to_string();
            let reply = format!(
                "NAMING REPLY RESULT=KEY_NOT_FOUND NAME={name}
"
            );
            tcp.write_all(reply.as_bytes()).await.unwrap();
        } else if cmd.starts_with("SESSION CREATE") {
            let id = pair(&cmd, "ID").unwrap().to_string();
            let given = pair(&cmd, "DESTINATION").unwrap().to_string();
            let private = if given == "TRANSIENT" {
                let mut r = router.lock().unwrap();
                r.next_seed += 1;
                keypair(r.next_seed).1
            } else {
                given
            };
            let dest = public_of(&private);
            router
                .lock()
                .unwrap()
                .sessions
                .insert(id.clone(), dest.clone());
            let reply = format!("SESSION STATUS RESULT=OK DESTINATION={private}\n");
            tcp.write_all(reply.as_bytes()).await.unwrap();
            // The session lives while this connection does, and resolves names on it.
            while let Some(cmd) = line(&mut tcp).await {
                router.lock().unwrap().log.push(cmd.clone());
                if cmd.starts_with("NAMING LOOKUP") {
                    let name = pair(&cmd, "NAME").unwrap().to_string();
                    let reply = match lookup(&router, &name) {
                        Some(dest) => format!(
                            "NAMING REPLY RESULT=OK NAME={name} VALUE={dest}
"
                        ),
                        None => format!(
                            "NAMING REPLY RESULT=KEY_NOT_FOUND NAME={name}
"
                        ),
                    };
                    tcp.write_all(reply.as_bytes()).await.unwrap();
                }
            }
            let mut r = router.lock().unwrap();
            r.sessions.remove(&id);
            r.acceptors.remove(&dest);
            return;
        } else if cmd.starts_with("STREAM ACCEPT") {
            let id = pair(&cmd, "ID").unwrap();
            let dest = router.lock().unwrap().sessions.get(id).cloned().unwrap();
            tcp.write_all(b"STREAM STATUS RESULT=OK\n").await.unwrap();
            router
                .lock()
                .unwrap()
                .acceptors
                .entry(dest)
                .or_default()
                .push_back(tcp);
            return;
        } else if cmd.starts_with("STREAM CONNECT") {
            let id = pair(&cmd, "ID").unwrap();
            let to = pair(&cmd, "DESTINATION").unwrap();
            let (from, acceptor) = {
                let mut r = router.lock().unwrap();
                let from = r.sessions.get(id).cloned().unwrap();
                (from, r.acceptors.get_mut(to).and_then(VecDeque::pop_front))
            };
            let Some(mut acceptor) = acceptor else {
                tcp.write_all(b"STREAM STATUS RESULT=CANT_REACH_PEER\n")
                    .await
                    .unwrap();
                continue;
            };
            tcp.write_all(b"STREAM STATUS RESULT=OK\n").await.unwrap();
            let peer = format!("{from} FROM_PORT=0 TO_PORT=0\n");
            acceptor.write_all(peer.as_bytes()).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut acceptor).await;
            return;
        }
    }
}

fn quick() -> I2pTimers {
    I2pTimers {
        reconnect_wait: Duration::from_millis(100),
        client_tunnel_retry: Duration::from_millis(100),
        server_tunnel_retry: Duration::from_millis(100),
        ..I2pTimers::default()
    }
}

/// A connectable hub learns a leaf's announce across I2P and opens a link that echoes. The
/// leaf dials the hub by its `.b32.i2p` name, so the lookup is exercised too.
#[tokio::test]
async fn a_hub_learns_a_leaf_over_i2p_and_links_to_it() {
    let (bridge, router) = fake_router().await;
    let hub = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[1u8; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let hub_key = PrivateKey::from_base64(&keypair(1).1).unwrap();
    let listener = hub
        .listen_i2p(I2pListen::new(bridge, hub_key.clone()))
        .unwrap();
    assert_eq!(listener.destination(), hub_key.destination());

    let leaf = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[2u8; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let mut peer = I2pPeer::new(bridge, "I2P", listener.destination().b32());
    peer.timers = quick();
    let leaf_iface = leaf.attach_i2p_peer(peer).unwrap();
    let name = DestinationName::new("leaf", ["i2p"]);
    leaf.register(name.clone(), b"i2p");
    let announcer = Arc::clone(&leaf);
    tokio::spawn(async move {
        for _ in 0..40 {
            announcer.announce(&name, b"i2p");
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    });
    let server = Arc::clone(&leaf);
    tokio::spawn(async move {
        while let Ok(mut stream) = server.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut reply = b"echo:".to_vec();
                    reply.extend_from_slice(&buf[..n]);
                    if stream.write_all(&reply).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    let announced = tokio::time::timeout(Duration::from_secs(10), hub.next_announcement())
        .await
        .expect("hub hears the leaf within 10 s")
        .unwrap();
    assert!(leaf.interface_online(leaf_iface));
    assert_eq!(
        hub.interface_count(),
        1,
        "one accepted stream, one interface"
    );

    let mut stream = hub
        .open(announced.destination, announced.identity)
        .await
        .unwrap();
    stream.write_all(b"over i2p").await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
        .await
        .expect("echo within 10 s")
        .unwrap();
    assert_eq!(&buf[..n], b"echo:over i2p");

    let log = router.lock().unwrap().log.clone();
    let lookup = format!("NAMING LOOKUP NAME={}", hub_key.destination().b32());
    assert!(
        log.contains(&lookup),
        "the leaf resolved the hub's b32 name"
    );
    let persistent = format!(" DESTINATION={}", hub_key.expose_base64());
    assert!(
        log.iter()
            .any(|l| l.starts_with("SESSION CREATE STYLE=STREAM ID=") && l.ends_with(&persistent)),
        "the hub's session is persistent, with no options: {log:?}"
    );
}

/// A silent far end gets the empty-frame keepalive after the quiet spell, is dropped after
/// the read deadline, and is redialed.
#[tokio::test]
async fn a_silent_peer_gets_keepalives_then_is_dropped_and_redialed() {
    let (bridge, _router) = fake_router().await;
    let far_key = PrivateKey::from_base64(&keypair(9).1).unwrap();
    let far = StreamSession::create(
        bridge,
        Key::Persistent(far_key.clone()),
        SessionOptions::default(),
    )
    .await
    .unwrap();

    let node = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[3u8; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let mut peer = I2pPeer::new(bridge, "I2P", far_key.destination().to_base64());
    peer.timers = I2pTimers {
        keepalive_after: Duration::from_millis(200),
        read_idle: Duration::from_millis(700),
        ..quick()
    };
    node.attach_i2p_peer(peer).unwrap();

    let (mut stream, from) = tokio::time::timeout(Duration::from_secs(5), far.accept())
        .await
        .expect("first stream")
        .unwrap();
    assert_ne!(
        &from,
        far_key.destination(),
        "the dialer speaks from its own session"
    );
    // Collect for longer than one quiet spell but shorter than two.
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(Duration::from_millis(350), async {
        let mut buf = [0u8; 1024];
        while let Ok(n) = stream.read(&mut buf).await {
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
        }
    })
    .await;
    // The tunnel synthesis frame, then nothing but the keepalive.
    assert_eq!(seen[0], 0x7E);
    let end = 1 + seen[1..].iter().position(|&b| b == 0x7E).unwrap();
    assert!(end > 2, "a non-empty first frame: {seen:02x?}");
    assert_eq!(
        &seen[end + 1..],
        [0x7E, 0x7E],
        "one empty-frame keepalive: {seen:02x?}"
    );

    // Stay silent: the dialer gives up on the stream and closes it.
    let mut buf = [0u8; 64];
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the read deadline closed the stream");

    // And dials again.
    let again = tokio::time::timeout(Duration::from_secs(5), far.accept()).await;
    assert!(
        again.is_ok_and(|a| a.is_ok()),
        "redialed after the deadline"
    );
}

/// A peer nobody accepts for stays offline while its dialer keeps retrying; the positive
/// control in the hub test above shows the same path connecting when someone does.
#[tokio::test]
async fn an_unreachable_peer_stays_offline_and_keeps_retrying() {
    let (bridge, router) = fake_router().await;
    let node = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[4u8; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let nobody = PrivateKey::from_base64(&keypair(7).1).unwrap();
    let mut peer = I2pPeer::new(bridge, "I2P", nobody.destination().to_base64());
    peer.timers = quick();
    let id = node.attach_i2p_peer(peer).unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(!node.interface_online(id));
    let attempts = router
        .lock()
        .unwrap()
        .log
        .iter()
        .filter(|l| l.starts_with("STREAM CONNECT"))
        .count();
    assert!(attempts >= 3, "kept redialing: {attempts}");
    // By default every redial starts from a fresh session: one per attempt, plus at most
    // one built for the attempt in flight.
    let sessions = count(&router, "SESSION CREATE");
    assert!(
        (attempts..=attempts + 1).contains(&sessions),
        "{sessions} sessions for {attempts} attempts"
    );

    // With the session kept, the same refusals reuse exactly one.
    let (bridge, router) = fake_router().await;
    let mut kept = I2pPeer::new(bridge, "I2P", nobody.destination().to_base64());
    kept.timers = quick();
    kept.fresh_session_per_redial = false;
    let kept = node.attach_i2p_peer(kept).unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(!node.interface_online(kept));
    assert!(count(&router, "STREAM CONNECT") >= 3);
    assert_eq!(count(&router, "SESSION CREATE"), 1);

    // No bridge at all: the endpoint is unharmed and the interface waits offline.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gone = Bridge::new(closed.local_addr().unwrap());
    drop(closed);
    let mut lost = I2pPeer::new(gone, "I2P", nobody.destination().to_base64());
    lost.timers = quick();
    let lost = node.attach_i2p_peer(lost).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!node.interface_online(lost));
    assert_eq!(node.interface_count(), 3);
}

/// Lines the fake router logged that start with `prefix`.
fn count(router: &Shared, prefix: &str) -> usize {
    let router = router.lock().unwrap();
    router.log.iter().filter(|l| l.starts_with(prefix)).count()
}

/// RNS's keyfile names, old-format preference, and generate-once storage.
#[tokio::test]
async fn keyfiles_follow_the_rns_layout_and_are_generated_once() {
    let (bridge, router) = fake_router().await;
    let storage = std::env::temp_dir().join(format!("retinue-i2p-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&storage);
    let transport = PrivateIdentity::from_secret_bytes(&[5u8; 64]).hash();

    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let name_hash = Sha256::digest(b"I2P");
    let mut both = name_hash.to_vec();
    both.extend_from_slice(&Sha256::digest(transport.as_slice()));
    let new = storage
        .join("i2p")
        .join(format!("{}.i2p", hex(&Sha256::digest(&both))));
    let old = storage
        .join("i2p")
        .join(format!("{}.i2p", hex(&Sha256::digest(name_hash))));

    let path = i2p_keyfile(&storage, "I2P", transport);
    assert_eq!(path, new);
    let key = load_or_generate_i2p_key(bridge, &path).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), key.expose_base64());
    let again = load_or_generate_i2p_key(bridge, &path).await.unwrap();
    assert_eq!(again, key);
    let generated = router
        .lock()
        .unwrap()
        .log
        .iter()
        .filter(|l| l.starts_with("DEST GENERATE SIGNATURE_TYPE=7"))
        .count();
    assert_eq!(generated, 1, "the stored key is reused");

    std::fs::write(&old, keypair(8).1).unwrap();
    assert_eq!(
        i2p_keyfile(&storage, "I2P", transport),
        old,
        "an old-format key wins"
    );
    std::fs::remove_dir_all(&storage).unwrap();
}

#[test]
fn an_i2p_section_parses_with_stream_ifac_defaults() {
    let config = RnsConfig::parse(
        "[interfaces]\n  [[I2P]]\n    type = I2PInterface\n    enabled = yes\n    \
         connectable = yes\n    peers = aaaa.b32.i2p, bbbb.b32.i2p\n    \
         networkname = net\n    passphrase = pass\n",
    )
    .unwrap();
    let section = config.interfaces[0].as_ref().unwrap();
    assert_eq!(section.name, "I2P");
    let Carrier::I2p(i2p) = &section.carrier else {
        panic!("not an I2P carrier: {:?}", section.carrier);
    };
    assert_eq!(i2p.peers, ["aaaa.b32.i2p", "bbbb.b32.i2p"]);
    assert!(i2p.connectable);
    assert_eq!(i2p.ifac.as_ref().map(|i| i.size()), Some(16));
    let bare = RnsConfig::parse("[interfaces]\n  [[I2P]]\n    type = I2PInterface\n").unwrap();
    let Carrier::I2p(bare) = &bare.interfaces[0].as_ref().unwrap().carrier else {
        panic!("not an I2P carrier");
    };
    assert!(bare.peers.is_empty() && !bare.connectable && bare.ifac.is_none());
}
