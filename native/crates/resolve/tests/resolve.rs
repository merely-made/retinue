// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! A reader holding only an address finds a holder and verifies, in process.
//!
//! Memory and Noise-over-TCP always run; p2panda and Reticulum run behind
//! their features.

use std::time::Duration;

use identity::Ed25519Keypair;
use muniment::MemoryBackend;
use murm_resolve::{
    AddressHost, Failure, Hint, Holder, LogAddress, MootCollection, NoiseTcpAdapter,
    ReleaseAddress, ResolveError, Resolver, TransportAdapter, release_alpn,
};
use stickleback::edition::{
    ContentRef, EditionBody, EditionId, EditionKind, EditionOperation, EditionWriter, Provenance,
    SigningKey, edition_topic,
};
use transport::memory::MemoryTransport;
use transport::security::SetupLimits;
use transport::sync_lane::SyncLane;
use transport::{PeerID, Transport};

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn key(seed: u8) -> Ed25519Keypair {
    Ed25519Keypair::from_seed([seed; 32])
}

fn id_of(key: &Ed25519Keypair) -> PeerID {
    PeerID::from_public_key(key.public_key())
}

struct Log {
    key: [u8; 32],
    editions: Vec<EditionOperation>,
}

fn log(editions: u8) -> Log {
    let signer = SigningKey::generate();
    let mut writer = EditionWriter::new(signer.clone());
    let editions = (1..=editions)
        .map(|seed| {
            writer
                .append(&EditionBody {
                    kind: EditionKind::Site,
                    content: ContentRef::Snapshot {
                        digest: EditionId([seed; 32]),
                    },
                    provenance: Provenance {
                        source: "resolve-test".into(),
                        created_ms: u64::from(seed),
                    },
                    fork_of: None,
                    hints: vec![],
                    sealed: None,
                })
                .unwrap()
        })
        .collect();
    Log {
        key: *signer.verifying_key().as_bytes(),
        editions,
    }
}

impl Log {
    fn address(&self, pin: Option<usize>) -> LogAddress {
        let address = LogAddress::new(self.key, "index.gmi");
        match pin {
            Some(index) => address.pinned(*self.editions[index].hash.as_bytes()),
            None => address,
        }
    }
}

async fn holder_of(logs: &[&Log]) -> Holder<MemoryBackend> {
    let holder = Holder::new(MemoryBackend::new());
    for log in logs {
        assert_eq!(holder.hold(&log.editions).await, log.editions.len());
    }
    holder
}

fn lane() -> SyncLane {
    SyncLane::new(release_alpn(), 4, SetupLimits::default())
}

/// Serve one memory session as `holder_key`, bounded so a refusal on the
/// reader's side cannot leave the test waiting.
async fn serve_memory(
    holder: &Holder<MemoryBackend>,
    transport: &MemoryTransport,
    holder_key: Ed25519Keypair,
) {
    let served = async {
        let accepted = transport.accept(release_alpn()).await.unwrap();
        lane()
            .serve_noise(
                &holder_key,
                accepted.into_stream(),
                |_| true,
                holder.session(),
            )
            .await
    };
    let _ = tokio::time::timeout(Duration::from_secs(10), served).await;
}

fn memory_pair() -> (MemoryTransport, MemoryTransport) {
    MemoryTransport::pair(id_of(&key(101)), id_of(&key(102)))
}

#[tokio::test]
async fn memory_reader_with_only_an_address_verifies_the_chain() {
    let site = log(3);
    let holder = holder_of(&[&site]).await;
    let (reader_t, holder_t) = memory_pair();
    let resolver = Resolver::new().with(
        TransportAdapter::new("memory", reader_t, key(1))
            .with_peers(vec![holder_t.local_peer_id()]),
    );
    let address = site.address(None);
    let (resolved, ()) = tokio::join!(
        resolver.resolve(&address, &[]),
        serve_memory(&holder, &holder_t, key(2))
    );
    let resolved = resolved.unwrap();
    assert_eq!(resolved.chain.editions().len(), 3);
    assert_eq!(resolved.selected.seq_num, 2);
    assert_eq!(resolved.path, "index.gmi");
    assert_eq!(resolved.via.0, "memory");
}

#[tokio::test]
async fn a_pinned_hash_returns_that_edition() {
    let site = log(3);
    let holder = holder_of(&[&site]).await;
    let (reader_t, holder_t) = memory_pair();
    let resolver = Resolver::new().with(
        TransportAdapter::new("memory", reader_t, key(1))
            .with_peers(vec![holder_t.local_peer_id()]),
    );
    let pinned = site.address(Some(1));
    let (resolved, ()) = tokio::join!(
        resolver.resolve(&pinned, &[]),
        serve_memory(&holder, &holder_t, key(2))
    );
    let resolved = resolved.unwrap();
    assert_eq!(resolved.selected.seq_num, 1);
    assert_eq!(resolved.selected.id.0, *site.editions[1].hash.as_bytes());
    assert_eq!(resolved.selected.body.content.digest(), [2; 32]);

    // An edition the holder lacks refuses; it is not replaced by the head.
    let other = log(2);
    let missing = LogAddress::new(site.key, "index.gmi").pinned(*other.editions[1].hash.as_bytes());
    let (reader_t, holder_t) = memory_pair();
    let resolver = Resolver::new().with(
        TransportAdapter::new("memory", reader_t, key(1))
            .with_peers(vec![holder_t.local_peer_id()]),
    );
    let (refused, ()) = tokio::join!(
        resolver.resolve(&missing, &[]),
        serve_memory(&holder, &holder_t, key(2))
    );
    let Err(ResolveError::Unresolved { attempts }) = refused else {
        panic!("a missing pin must not resolve");
    };
    assert!(matches!(attempts[0].result, Err(Failure::PinMissing)));
}

#[tokio::test]
async fn a_hint_to_a_holder_of_a_different_log_refuses() {
    let wanted = log(2);
    let served = log(2);
    let holder = holder_of(&[&served]).await;
    let (reader_t, holder_t) = memory_pair();
    let hints = [Hint::Peer(holder_t.local_peer_id())];
    let resolver = Resolver::new().with(TransportAdapter::new("memory", reader_t, key(1)));
    let address = wanted.address(None);
    let (refused, ()) = tokio::join!(
        resolver.resolve(&address, &hints),
        serve_memory(&holder, &holder_t, key(2))
    );
    let Err(ResolveError::Unresolved { attempts }) = refused else {
        panic!("a hinted holder of another log must not resolve");
    };
    assert!(
        matches!(attempts[0].result, Err(Failure::NotHeld)),
        "{attempts:?}"
    );
}

/// A holder that says it holds the wanted log and sends another's editions
/// under the wanted topic.
mod hostile {
    use murm_sync::{
        StreamSession, StreamSync, StreamSyncError, StreamSyncLimits, StreamSyncOutcome,
    };
    use stickleback::MunimentStore;
    use stickleback::edition::EditionExt;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    use super::*;

    pub struct Liar {
        pub store: MunimentStore<MemoryBackend, EditionExt>,
        pub wanted: [u8; 32],
    }

    impl StreamSession for Liar {
        async fn run<St>(self, mut stream: St) -> Result<StreamSyncOutcome, StreamSyncError>
        where
            St: AsyncRead + AsyncWrite + Unpin,
        {
            let mut request = [0u8; 34];
            stream.read_exact(&mut request).await.unwrap();
            stream.write_all(&[1]).await.unwrap();
            stream.flush().await.unwrap();
            StreamSync::<_, u64, EditionExt, _>::new(
                edition_topic(&self.wanted),
                self.store,
                StreamSyncLimits::default(),
                |_| std::future::ready(false),
            )
            .run(stream)
            .await
        }
    }
}

#[tokio::test]
async fn a_holder_serving_another_logs_editions_under_the_wanted_name_is_declined() {
    let wanted = log(2);
    let foreign = log(2);
    let store = stickleback::MunimentStore::new(MemoryBackend::new());
    for edition in &foreign.editions {
        store
            .insert_indexed_operation(&edition_topic(&wanted.key), edition, &0u64)
            .await
            .unwrap();
    }
    let liar = hostile::Liar {
        store,
        wanted: wanted.key,
    };
    let (reader_t, holder_t) = memory_pair();
    let resolver = Resolver::new().with(
        TransportAdapter::new("memory", reader_t, key(1))
            .with_peers(vec![holder_t.local_peer_id()]),
    );
    let address = wanted.address(None);
    let serve = async {
        let accepted = holder_t.accept(release_alpn()).await.unwrap();
        let _ = lane()
            .serve_noise(&key(2), accepted.into_stream(), |_| true, liar)
            .await;
    };
    let (refused, ()) = tokio::join!(resolver.resolve(&address, &[]), serve);
    let Err(ResolveError::Unresolved { attempts }) = refused else {
        panic!("another log's editions must not resolve");
    };
    assert!(
        matches!(attempts[0].result, Err(Failure::Mismatch { declined: 2 })),
        "{attempts:?}"
    );
}

#[tokio::test]
async fn noise_tcp_holder_from_a_hint_resolves_and_the_wrong_key_refuses() {
    let site = log(2);
    let holder = holder_of(&[&site]).await;
    let server_key = key(5);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve = async {
        // Two dials: the one that names a wrong key never gets past Noise.
        for _ in 0..2 {
            let Ok(Ok((tcp, _))) =
                tokio::time::timeout(Duration::from_secs(10), listener.accept()).await
            else {
                return;
            };
            let _ = lane()
                .serve_noise(&server_key, tcp, |_| true, holder.session())
                .await;
        }
    };
    let resolver = Resolver::new().with(NoiseTcpAdapter::new(key(6)));
    let address = site.address(None);
    let wrong = Hint::parse(&format!(
        "noise+tcp://{addr}/{}",
        hex(&id_of(&key(7)).to_bytes())
    ))
    .unwrap();
    let right = Hint::parse(&format!(
        "noise+tcp://{addr}/{}",
        hex(&id_of(&server_key).to_bytes())
    ))
    .unwrap();
    let (resolved, ()) = tokio::join!(
        async {
            let refused = resolver.resolve(&address, &[wrong]).await;
            assert!(matches!(refused, Err(ResolveError::Unresolved { .. })));
            resolver.resolve(&address, &[right]).await
        },
        serve
    );
    assert_eq!(resolved.unwrap().selected.seq_num, 1);
}

#[tokio::test]
async fn the_governed_form_is_handed_to_gemot_not_resolved() {
    /// A governed address as a grammar crate would hand it over.
    struct Governed;
    impl ReleaseAddress for Governed {
        fn host(&self) -> AddressHost {
            AddressHost::MootId([7; 32])
        }
        fn collection(&self) -> Option<MootCollection> {
            Some(MootCollection {
                moot_id: [7; 32],
                collection_id: [8; 32],
                page: "index.gmi".to_owned(),
            })
        }
        fn pin(&self) -> Option<[u8; 32]> {
            None
        }
        fn path(&self) -> &str {
            "8888/index.gmi"
        }
    }
    let address = Governed;
    let refused = Resolver::new().resolve(&address, &[]).await;
    let Err(ResolveError::Governed {
        moot_id,
        collection_id,
        page,
    }) = refused
    else {
        panic!("governed addresses are not resolved here");
    };
    assert_eq!(
        (moot_id, collection_id, page.as_str()),
        ([7; 32], [8; 32], "index.gmi")
    );
}

#[test]
fn hints_parse_and_unknown_schemes_refuse() {
    let peer = hex(&id_of(&key(9)).to_bytes());
    assert!(matches!(
        Hint::parse(&format!("peer:{peer}")),
        Ok(Hint::Peer(_))
    ));
    assert!(matches!(Hint::parse("i2p:abc.b32.i2p"), Ok(Hint::I2p(_))));
    assert!(matches!(Hint::parse("onion:abc.onion"), Ok(Hint::Onion(_))));
    for bad in [
        "http://example.com",
        "peer:abc",
        "noise+tcp://nowhere/x",
        "onion:",
    ] {
        assert!(Hint::parse(bad).is_err(), "{bad}");
    }
}

#[cfg(feature = "p2panda")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2panda_reader_finds_a_topic_tagged_holder() {
    use murm_resolve::P2pandaAdapter;
    use transport::P2pandaTransport;

    let site = log(3);
    let holder = holder_of(&[&site]).await;
    let alpn = release_alpn();
    let build = |seed: u8| {
        let alpn = alpn.clone();
        async move {
            P2pandaTransport::builder_from_seed_ref(&[seed; 32])
                .alpns(vec![alpn])
                .bind()
                .await
                .unwrap()
        }
    };
    let (reader_t, holder_t) = (build(41).await, build(42).await);
    // Stand-in for discovery: the reader's book learns the holder and its topic.
    let holder_peer = holder_t.local_peer_id();
    reader_t
        .add_peer(holder_t.endpoint_addr().await.unwrap())
        .await
        .unwrap();
    reader_t
        .set_topics(holder_peer, &[*edition_topic(&site.key).as_bytes()])
        .await
        .unwrap();
    let serve = async {
        let accepted = holder_t.accept(alpn.clone()).await.unwrap();
        lane()
            .serve_native(accepted, |_| true, holder.session())
            .await
    };
    let resolver = Resolver::new().with(P2pandaAdapter::new(reader_t));
    let address = site.address(Some(0));
    let (resolved, _) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(resolver.resolve(&address, &[]), serve)
    })
    .await
    .expect("p2panda resolve timed out");
    let resolved = resolved.unwrap();
    assert_eq!(resolved.via.0, "p2panda");
    assert_eq!(
        (resolved.selected.seq_num, resolved.chain.editions().len()),
        (0, 3)
    );
}

#[cfg(feature = "reticulum")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reticulum_reader_finds_the_holder_that_announced_the_keys_name() {
    use transport::{ReticulumInterface, ReticulumTransport};

    let site = log(2);
    let holder = holder_of(&[&site]).await;
    let alpn = murm_resolve::key_alpn(&site.key);
    let port = 47_863u16;
    let (holder_carrier, reader_carrier) = (key(51), key(52));
    let holder_t = ReticulumTransport::builder(&holder_carrier)
        .network_stamps(enforcing_low_cost())
        .alpns(vec![alpn.clone()])
        .interfaces(vec![ReticulumInterface::TcpServer {
            bind: format!("127.0.0.1:{port}").parse().unwrap(),
        }])
        .announce_interval(Duration::from_millis(500))
        .bind()
        .await
        .unwrap();
    let reader_t = ReticulumTransport::builder(&reader_carrier)
        .network_stamps(enforcing_low_cost())
        .watch_alpns(vec![alpn.clone()])
        .interfaces(vec![ReticulumInterface::TcpClient {
            addr: format!("127.0.0.1:{port}").parse().unwrap(),
        }])
        .announce_interval(Duration::from_millis(500))
        .connect_timeout(Duration::from_secs(20))
        .bind()
        .await
        .unwrap();
    let holder_key = key(53);
    let serve = async {
        let accepted = tokio::time::timeout(Duration::from_secs(40), holder_t.accept(alpn.clone()))
            .await
            .expect("accept timed out")
            .unwrap();
        lane_for(&alpn)
            .serve_noise(
                &holder_key,
                accepted.into_stream(),
                |_| true,
                holder.session(),
            )
            .await
    };
    let resolver = Resolver::new()
        .with(murm_resolve::reticulum_adapter(reader_t, key(54)).waiting(Duration::from_secs(20)));
    let address = site.address(None);
    let (resolved, _) = tokio::time::timeout(Duration::from_secs(90), async {
        tokio::join!(resolver.resolve(&address, &[]), serve)
    })
    .await
    .expect("reticulum resolve timed out");
    let resolved = resolved.unwrap();
    assert_eq!(resolved.via.0, "reticulum");
    assert_eq!(resolved.selected.seq_num, 1);
}

/// Both ends stamp and enforce, as an RNS 1.6.2 network will; a low destination value
/// keeps mining quick in debug builds. Path and link stamps keep stock's default costs.
#[cfg(feature = "reticulum")]
fn enforcing_low_cost() -> transport::NetworkStampPolicy {
    transport::NetworkStampPolicy {
        enforce: true,
        destination_cost: transport::StampCost::from_integer(8),
        destination_value: transport::StampCost::from_integer(8),
        ..Default::default()
    }
}

#[cfg(feature = "reticulum")]
fn lane_for(alpn: &transport::Alpn) -> SyncLane {
    SyncLane::new(alpn.clone(), 4, SetupLimits::default())
}
