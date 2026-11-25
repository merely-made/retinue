//! The three-party path the attenuated WN2 bench run depends on, with no hardware:
//!
//! ```text
//! requester Endpoint ── radio ── Node<32, 8, 4> relay ── radio ── responder Endpoint
//! ```
//!
//! Each endpoint drives a direct-PHY style packet radio through `retinue::iface::tulle::drive`,
//! as `examples/transit_peer.rs` does, with an in-memory radio in place of the serial link and
//! the 255-byte frame limit. The node is driven the way the board's `NodeChannel` drives it:
//! each whole frame ingested on its one radio interface, every action performed, and polled
//! on its own deadline and a beat. The air: the node hears both endpoints and both hear the
//! node; the endpoints never hear each other, so any exchange between them crossed the node.
//!
//! The payload is the D16 form, 256 random bytes each way over the link's best-effort stream;
//! the exchange mirrors `transit_peer` (an example cannot be imported). The control turns the
//! node's relaying off and requires the requester to fail and the responder to accept nothing.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use retinue::node::{Action, Actions, Node, TransportConfig, TransportCounters};
use retinue::packet::Packet;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tulle::link::Received;
use tulle::radio_io::PacketRadio;
use tulle::serial::TransmitError;

/// One direct-PHY frame (`selvage::MAX_RADIO_FRAME_LEN`), and the link MTU both ends offer.
const FRAME: usize = 255;
const PAYLOAD: usize = 256;
const RADIO: retinue::node::InterfaceId = 0;

const REQUESTER_SEED: [u8; 64] = [0x24; 64];
const RESPONDER_SEED: [u8; 64] = [0x42; 64];
const NODE_SEED: [u8; 64] = [0x5a; 64];

/// Which endpoint put a frame on the air. Only the node hears either of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Requester,
    Responder,
}

/// An endpoint's radio: what it sends reaches the node alone; it hears only the node.
struct EndpointRadio {
    side: Side,
    to_node: mpsc::UnboundedSender<(Side, Vec<u8>)>,
    from_node: mpsc::UnboundedReceiver<Vec<u8>>,
    /// Set only by the direct-air case: the other endpoint hears this one too.
    to_peer: Option<mpsc::UnboundedSender<Vec<u8>>>,
    oversize: Arc<AtomicUsize>,
}

#[allow(clippy::manual_async_fn)]
impl PacketRadio for EndpointRadio {
    fn max_frame_len(&self) -> usize {
        FRAME
    }

    fn send_frame(
        &self,
        frame: Vec<u8>,
    ) -> impl Future<Output = Result<Duration, TransmitError>> + Send {
        let to_node = self.to_node.clone();
        let side = self.side;
        let oversize = Arc::clone(&self.oversize);
        let to_peer = self.to_peer.clone();
        async move {
            if let Some(peer) = to_peer {
                let _ = peer.send(frame.clone());
            }
            if frame.len() > FRAME {
                oversize.fetch_add(1, Ordering::Relaxed);
            }
            to_node
                .send((side, frame))
                .map_err(|_| TransmitError::Stopped)?;
            Ok(Duration::from_millis(1))
        }
    }

    fn recv_frame(&mut self) -> impl Future<Output = Option<Received>> + Send {
        async move {
            self.from_node.recv().await.map(|frame| Received {
                frame,
                rssi_dbm: -60,
                snr_db: 7.0,
            })
        }
    }
}

/// What the node did, for assertions after the run.
#[derive(Default)]
struct NodeLog {
    heard: [usize; 2],
    sent: usize,
    undecoded: usize,
    oversize: usize,
    counters: Option<TransportCounters>,
}

/// The node as `NodeChannel` runs it: ingest each whole frame, perform the sends on its one
/// radio (both endpoints hear every one), and poll on its deadline and a beat.
async fn run_node(
    mut node: Node<32, 8, 4>,
    mut inbox: mpsc::UnboundedReceiver<(Side, Vec<u8>)>,
    to_requester: mpsc::UnboundedSender<Vec<u8>>,
    to_responder: mpsc::UnboundedSender<Vec<u8>>,
    log: Arc<Mutex<NodeLog>>,
) {
    const BEAT: Duration = Duration::from_millis(500);
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let perform = |actions: Actions<8>, log: &Mutex<NodeLog>| {
        for action in actions {
            if let Action::Send { packet, .. } = action {
                let bytes = packet.encode();
                let mut log = log.lock().unwrap();
                if bytes.len() > FRAME {
                    log.oversize += 1;
                    continue;
                }
                log.sent += 1;
                let _ = to_requester.send(bytes.clone());
                let _ = to_responder.send(bytes);
            }
        }
    };
    loop {
        let wake = node
            .next_deadline()
            .map(|at| Duration::from_millis(at.saturating_sub(now())))
            .unwrap_or(BEAT)
            .min(BEAT);
        tokio::select! {
            frame = inbox.recv() => {
                let Some((side, frame)) = frame else { break };
                log.lock().unwrap().heard[side as usize] += 1;
                match Packet::decode(&frame) {
                    Ok(packet) => {
                        let actions = node.ingest(RADIO, &packet, now());
                        perform(actions, &log);
                    }
                    Err(_) => log.lock().unwrap().undecoded += 1,
                }
            }
            _ = tokio::time::sleep(wake) => {
                let actions = node.poll(now(), RADIO, None);
                perform(actions, &log);
            }
        }
        log.lock().unwrap().counters = Some(node.transport_counters());
    }
}

struct Bench {
    requester: Endpoint,
    responder: Arc<Endpoint>,
    node_log: Arc<Mutex<NodeLog>>,
    endpoint_oversize: Arc<AtomicUsize>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Bench {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn bench(transport: TransportConfig, direct: bool) -> Bench {
    let requester = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&REQUESTER_SEED),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let responder = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&RESPONDER_SEED),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    requester.set_link_mtu(FRAME as u32);
    responder.set_link_mtu(FRAME as u32);

    let (to_node, inbox) = mpsc::unbounded_channel();
    let (to_requester, requester_rx) = mpsc::unbounded_channel();
    let (to_responder, responder_rx) = mpsc::unbounded_channel();
    let oversize = Arc::new(AtomicUsize::new(0));
    let requester_radio = EndpointRadio {
        side: Side::Requester,
        to_node: to_node.clone(),
        from_node: requester_rx,
        to_peer: direct.then(|| to_responder.clone()),
        oversize: Arc::clone(&oversize),
    };
    let responder_radio = EndpointRadio {
        side: Side::Responder,
        to_node,
        from_node: responder_rx,
        to_peer: direct.then(|| to_requester.clone()),
        oversize: Arc::clone(&oversize),
    };

    let node = Node::<32, 8, 4>::new(
        PrivateIdentity::from_secret_bytes(&NODE_SEED),
        DestinationName::new("retinue", ["node"]).name_hash(),
    )
    .with_transport_config(transport);
    let node_log = Arc::new(Mutex::new(NodeLog::default()));
    let tasks = vec![
        tokio::spawn(run_node(
            node,
            inbox,
            to_requester,
            to_responder,
            Arc::clone(&node_log),
        )),
        {
            let interface = requester.attach_interface();
            tokio::spawn(async move {
                let _ = drive(interface, requester_radio).await;
            })
        },
        {
            let interface = responder.attach_interface();
            tokio::spawn(async move {
                let _ = drive(interface, responder_radio).await;
            })
        },
    ];
    Bench {
        requester,
        responder,
        node_log,
        endpoint_oversize: oversize,
        tasks,
    }
}

fn responder_name() -> DestinationName {
    DestinationName::new("retinue", ["transit"])
}

fn responder_dest() -> AddressHash {
    responder_name().destination_hash(PrivateIdentity::from_secret_bytes(&RESPONDER_SEED).public())
}

fn node_transport_id() -> AddressHash {
    PrivateIdentity::from_secret_bytes(&NODE_SEED).hash()
}

fn random_payload() -> Vec<u8> {
    let mut payload = vec![0_u8; PAYLOAD];
    getrandom::fill(&mut payload).expect("entropy");
    payload
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The responder side of `transit_peer`: register, announce, and answer each inbound link
/// with one exchange. Returns how many links it accepted and the hashes it saw.
fn spawn_responder(
    responder: Arc<Endpoint>,
    window: Duration,
) -> tokio::task::JoinHandle<(usize, Vec<([u8; 32], [u8; 32])>)> {
    tokio::spawn(async move {
        let name = responder_name();
        // `register` announces; the timed ones follow an interval later.
        responder.register(name.clone(), b"transit");
        let deadline = tokio::time::Instant::now() + window;
        let mut next_announce = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut accepted = 0;
        let mut exchanges = Vec::new();
        while tokio::time::Instant::now() < deadline {
            if tokio::time::Instant::now() >= next_announce {
                responder.announce(&name, b"transit");
                next_announce = tokio::time::Instant::now() + Duration::from_secs(5);
            }
            let wait = next_announce.min(deadline) - tokio::time::Instant::now();
            let Ok(Ok(mut stream)) = tokio::time::timeout(wait, responder.accept()).await else {
                continue;
            };
            accepted += 1;
            let mut received = vec![0_u8; PAYLOAD];
            if tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut received))
                .await
                .is_ok_and(|read| read.is_ok())
            {
                let reply = random_payload();
                stream.write_all(&reply).await.expect("reply write");
                stream.flush().await.expect("reply flush");
                exchanges.push((sha256(&received), sha256(&reply)));
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        (accepted, exchanges)
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requester_reaches_a_responder_it_cannot_hear_through_the_node() {
    let bench = bench(TransportConfig::transit(), false);
    let responder = spawn_responder(Arc::clone(&bench.responder), Duration::from_secs(25));
    let dest = responder_dest();

    // The responder's announce reaches the requester only as the node's rebroadcast.
    let announce = loop {
        let announce =
            tokio::time::timeout(Duration::from_secs(15), bench.requester.next_announcement())
                .await
                .expect("the node rebroadcast the responder's announce")
                .expect("requester live");
        if announce.destination == dest {
            break announce;
        }
    };
    assert_eq!(
        announce.transport,
        Some(node_transport_id()),
        "the announce the requester used names the node as its transport"
    );
    assert!(announce.hops >= 1, "it travelled a hop: {}", announce.hops);
    let route = bench
        .requester
        .route_facts_at(Instant::now())
        .into_iter()
        .find(|route| route.destination == dest)
        .expect("a route to the responder");
    assert_eq!(
        route.transport,
        Some(node_transport_id()),
        "next hop is the node"
    );

    let mut stream = tokio::time::timeout(
        Duration::from_secs(15),
        bench.requester.open(dest, announce.identity),
    )
    .await
    .expect("link proof returned through the node")
    .expect("link opened");
    let sent = random_payload();
    stream.write_all(&sent).await.expect("write");
    stream.flush().await.expect("flush");
    let mut received = vec![0_u8; PAYLOAD];
    tokio::time::timeout(Duration::from_secs(15), stream.read_exact(&mut received))
        .await
        .expect("the responder's 256 bytes crossed the node")
        .expect("read");

    let (accepted, exchanges) = responder.await.expect("responder task");
    assert_eq!(accepted, 1);
    assert_eq!(exchanges.len(), 1, "the responder completed one exchange");
    let (responder_received, responder_sent) = exchanges[0];
    assert_eq!(
        responder_received,
        sha256(&sent),
        "requester to responder, byte-exact"
    );
    assert_eq!(
        responder_sent,
        sha256(&received),
        "responder to requester, byte-exact"
    );

    let log = bench.node_log.lock().unwrap();
    let counters = log.counters.expect("node ran");
    assert!(
        counters.forwarded_announces > 0,
        "forwarded announces moved"
    );
    assert!(counters.forwarded_packets > 0, "forwarded packets moved");
    assert!(
        log.heard[0] > 0 && log.heard[1] > 0,
        "the node heard both ends"
    );
    assert_eq!(log.undecoded, 0);
    assert_eq!(log.oversize, 0, "every node frame fit 255 bytes");
    assert_eq!(bench.endpoint_oversize.load(Ordering::Relaxed), 0);
    // By construction an endpoint's frames reach only the node, so nothing crossed directly.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_relaying_off_the_requester_fails_and_the_responder_accepts_nothing() {
    let off = TransportConfig {
        relay_announces: false,
        relay_packets: false,
        ..TransportConfig::transit()
    };
    let bench = bench(off, false);
    let responder = spawn_responder(Arc::clone(&bench.responder), Duration::from_secs(20));
    let dest = responder_dest();

    let heard = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match bench.requester.next_announcement().await {
                Ok(announce) if announce.destination == dest => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(!heard, "no relayed announce with relaying off");

    // Even knowing the responder's identity, a link request goes nowhere.
    let identity = *PrivateIdentity::from_secret_bytes(&RESPONDER_SEED).public();
    let opened = tokio::time::timeout(
        Duration::from_secs(10),
        bench.requester.open(dest, identity),
    )
    .await;
    assert!(
        !matches!(opened, Ok(Ok(_))),
        "the requester must not link with relaying off"
    );

    let (accepted, _) = responder.await.expect("responder task");
    assert_eq!(accepted, 0, "the responder accepted nothing");
    let log = bench.node_log.lock().unwrap();
    let counters = log.counters.expect("node ran");
    assert_eq!(counters.forwarded_announces, 0);
    assert_eq!(counters.forwarded_packets, 0);
    assert!(
        log.heard[1] > 0,
        "the node did hear the responder's announces"
    );
}

/// The open bench observation, settled: with a direct path heard first, the node's
/// rebroadcast of the same announce (same random blob, one more hop) is rejected as a replay
/// and the route stays direct. Stock RNS 1.6.0 does the same (`Transport.py` 2517-2577: a
/// higher hop count with a blob already in the path's list does not replace a live path).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requester_that_hears_the_responder_directly_drops_the_rebroadcast() {
    let bench = bench(TransportConfig::transit(), true);
    let responder = Arc::clone(&bench.responder);
    let name = responder_name();
    responder.register(name.clone(), b"transit");
    let dest = responder_dest();

    let announce =
        tokio::time::timeout(Duration::from_secs(5), bench.requester.next_announcement())
            .await
            .expect("the direct announce arrived")
            .expect("requester live");
    assert_eq!(announce.destination, dest);
    assert_eq!(announce.transport, None, "heard directly first");

    // Wait until the node has rebroadcast it, then for the copy to reach the requester.
    let relayed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let forwarded = bench
                .node_log
                .lock()
                .unwrap()
                .counters
                .is_some_and(|c| c.forwarded_announces > 0);
            if forwarded {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(relayed.is_ok(), "the node rebroadcast the announce");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Every announce surfaced to the requester came straight from the responder; the node's
    // copy, one hop further, never does.
    while let Ok(next) = tokio::time::timeout(
        Duration::from_millis(500),
        bench.requester.next_announcement(),
    )
    .await
    {
        let next = next.expect("requester live");
        assert_eq!(
            next.transport, None,
            "a relayed copy was surfaced: {next:?}"
        );
    }
    assert_eq!(
        bench
            .requester
            .routing_counters()
            .freshness_replays_rejected,
        1,
        "the rebroadcast was refused as a replay of the direct copy"
    );
    let route = bench
        .requester
        .route_facts_at(Instant::now())
        .into_iter()
        .find(|route| route.destination == dest)
        .expect("a route to the responder");
    assert_eq!(route.transport, None, "the route stays direct");
}
