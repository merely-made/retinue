// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! Catch-up sync through the shared setup gate, one test per carrier.
//!
//! Each pairs two in-process peers holding different halves of one log set and
//! requires both to end with all of it. Memory and Reticulum run Noise over the
//! carrier's stream, TCP runs Noise over a loopback socket, and p2panda runs
//! native iroh security. Reticulum is behind its own feature.

#![cfg(feature = "sync-lane")]

use std::collections::HashSet;
use std::time::Duration;

use identity::Ed25519Keypair;
use muniment::MemoryBackend;
use murm_sync::{StreamSession, StreamSync, StreamSyncLimits, StreamSyncOutcome, processor_accept};
use p2panda_core::{Body, Hash, Header, Operation, SigningKey, Topic, VerifyingKey};
use stickleback::{
    Admission, MunimentStore, OperationPolicy, OperationProcessor, Reject, StoreTarget,
};
use transport::memory::MemoryTransport;
use transport::security::SetupLimits;
use transport::sync_lane::{SyncLane, SyncLaneError};
use transport::{Alpn, PeerID, Transport};

type Store = MunimentStore<MemoryBackend, ()>;

const PROTOCOL: &str = "mere/release-sync/v1";

#[derive(Clone)]
struct Policy {
    topic: Topic,
    authors: HashSet<VerifyingKey>,
}

impl OperationPolicy<()> for Policy {
    type LogId = u64;

    fn admit(&self, operation: &Operation<()>) -> Result<Admission<u64>, Reject> {
        if !self.authors.contains(&operation.header.verifying_key) {
            return Err(Reject::new("author", "not a releaser"));
        }
        Ok(Admission::keep(StoreTarget::new(self.topic.clone(), 0)))
    }
}

struct Peer {
    topic: Topic,
    store: Store,
    processor: OperationProcessor<MemoryBackend, (), Policy>,
}

impl Peer {
    fn new(topic: &Topic, authors: &[&SigningKey]) -> Self {
        let store = Store::new(MemoryBackend::new());
        let policy = Policy {
            topic: topic.clone(),
            authors: authors.iter().map(|key| key.verifying_key()).collect(),
        };
        Self {
            topic: topic.clone(),
            processor: OperationProcessor::new(store.clone(), policy),
            store,
        }
    }

    async fn author(&self, key: &SigningKey, count: u32) {
        let mut backlink: Option<Hash> = None;
        for seq in 0..count {
            let payload = format!("release {seq}");
            let header = Header::<()>::builder()
                .body(payload.as_bytes())
                .seq_num(seq)
                .backlink(backlink)
                .build(key, ());
            let op = Operation {
                hash: header.hash(),
                header,
                body: Some(Body::from_bytes(payload.as_bytes())),
            };
            backlink = Some(op.hash);
            self.store
                .insert_indexed_operation(&self.topic, &op, &0u64)
                .await
                .unwrap();
        }
    }

    fn session(&self) -> impl StreamSession {
        StreamSync::<_, u64, (), _>::new(
            self.topic.clone(),
            self.store.clone(),
            StreamSyncLimits::new(1 << 20, 1 << 26, Duration::from_secs(30)).unwrap(),
            processor_accept(self.processor.clone()),
        )
    }

    async fn count(&self) -> usize {
        self.store.operation_count().await.unwrap()
    }
}

/// Two peers, each holding operations the other lacks.
async fn halves() -> (Peer, Peer) {
    let topic = Topic::random();
    let (ada, bea) = (SigningKey::generate(), SigningKey::generate());
    let (left, right) = (
        Peer::new(&topic, &[&ada, &bea]),
        Peer::new(&topic, &[&ada, &bea]),
    );
    left.author(&ada, 3).await;
    right.author(&bea, 2).await;
    (left, right)
}

async fn converged(left: &Peer, right: &Peer) {
    assert_eq!(left.count().await, 5);
    assert_eq!(right.count().await, 5);
}

fn lane() -> SyncLane {
    SyncLane::new(Alpn::new(PROTOCOL), 4, SetupLimits::default())
}

fn key(seed: u8) -> Ed25519Keypair {
    Ed25519Keypair::from_seed([seed; 32])
}

fn id_of(key: &Ed25519Keypair) -> PeerID {
    PeerID::from_public_key(key.public_key())
}

fn done(outcome: Result<StreamSyncOutcome, SyncLaneError>) -> StreamSyncOutcome {
    outcome.expect("lane session")
}

#[tokio::test]
async fn memory_peers_converge_through_the_gate() {
    let (left, right) = halves().await;
    let (dialer_key, server_key) = (key(1), key(2));
    let (mem_a, mem_b) = MemoryTransport::pair(id_of(&key(11)), id_of(&key(12)));
    let (server_lane, dial_lane) = (lane(), lane());

    let server = async {
        let accepted = mem_b.accept(Alpn::new(PROTOCOL)).await.unwrap();
        server_lane
            .serve_noise(
                &server_key,
                accepted.into_stream(),
                |_| true,
                right.session(),
            )
            .await
    };
    let dial = dial_lane.dial_noise(
        &mem_a,
        mem_b.local_peer_id(),
        &dialer_key,
        id_of(&server_key),
        left.session(),
    );
    let (served, dialed) = tokio::join!(server, dial);
    let (served, dialed) = (done(served), done(dialed));
    assert_eq!((dialed.accepted, served.accepted), (2, 3));
    converged(&left, &right).await;

    let stats = server_lane.stats();
    assert_eq!((stats.admitted, stats.pending), (1, 0));
}

#[tokio::test]
async fn noise_over_loopback_tcp_converges_from_an_explicit_address() {
    let (left, right) = halves().await;
    let (dialer_key, server_key) = (key(3), key(4));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (server_lane, dial_lane) = (lane(), lane());

    let server = async {
        let (tcp, _) = listener.accept().await.unwrap();
        server_lane
            .serve_noise(&server_key, tcp, |_| true, right.session())
            .await
    };
    let dial = dial_lane.dial_noise_tcp(addr, &dialer_key, id_of(&server_key), left.session());
    let (served, dialed) = tokio::join!(server, dial);
    assert_eq!((done(dialed).accepted, done(served).accepted), (2, 3));
    converged(&left, &right).await;
}

#[tokio::test]
async fn an_unexpected_responder_key_is_refused_before_any_sync() {
    let (left, right) = halves().await;
    let (dialer_key, server_key, wrong) = (key(5), key(6), key(7));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (server_lane, dial_lane) = (lane(), lane());

    let server = async {
        let (tcp, _) = listener.accept().await.unwrap();
        server_lane
            .serve_noise(&server_key, tcp, |_| true, right.session())
            .await
    };
    let dial = dial_lane.dial_noise_tcp(addr, &dialer_key, id_of(&wrong), left.session());
    let (_, dialed) = tokio::join!(server, dial);
    assert!(matches!(dialed, Err(SyncLaneError::Setup(_))), "{dialed:?}");
    assert_eq!((left.count().await, right.count().await), (3, 2));
}

#[tokio::test]
async fn the_owner_rule_refuses_an_authenticated_peer() {
    let (left, right) = halves().await;
    let (dialer_key, server_key) = (key(8), key(9));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (server_lane, dial_lane) = (lane(), lane());

    let server = async {
        let (tcp, _) = listener.accept().await.unwrap();
        server_lane
            .serve_noise(&server_key, tcp, |_| false, right.session())
            .await
    };
    let dial = dial_lane.dial_noise_tcp(addr, &dialer_key, id_of(&server_key), left.session());
    let (served, _) = tokio::join!(server, dial);
    assert!(matches!(served, Err(SyncLaneError::Refused)), "{served:?}");
    assert_eq!(right.count().await, 2);
    assert_eq!(server_lane.stats().admission_refused, 1);
}

#[tokio::test]
async fn an_out_of_policy_operation_is_declined_over_noise() {
    let topic = Topic::random();
    let (ada, outsider) = (SigningKey::generate(), SigningKey::generate());
    let honest = Peer::new(&topic, &[&ada]);
    let hostile = Peer::new(&topic, &[&ada, &outsider]);
    hostile.author(&ada, 1).await;
    hostile.author(&outsider, 2).await;

    let (dialer_key, server_key) = (key(13), key(14));
    let (mem_a, mem_b) = MemoryTransport::pair(id_of(&key(15)), id_of(&key(16)));
    let (server_lane, dial_lane) = (lane(), lane());
    let server = async {
        let accepted = mem_b.accept(Alpn::new(PROTOCOL)).await.unwrap();
        server_lane
            .serve_noise(
                &server_key,
                accepted.into_stream(),
                |_| true,
                honest.session(),
            )
            .await
    };
    let dial = dial_lane.dial_noise(
        &mem_a,
        mem_b.local_peer_id(),
        &dialer_key,
        id_of(&server_key),
        hostile.session(),
    );
    let (served, _) = tokio::join!(server, dial);
    let served = done(served);
    assert_eq!((served.accepted, served.declined), (1, 2));
    assert_eq!(honest.count().await, 1);
}

#[cfg(feature = "reticulum")]
mod reticulum {
    use transport::{ReticulumInterface, ReticulumTransport};

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reticulum_peers_converge_through_the_gate() {
        let (left, right) = halves().await;
        let alpn = Alpn::new(PROTOCOL);
        let port = 47_861u16;
        let (server_kp, client_kp) = (key(21), key(22));
        let (dialer_key, server_key) = (key(23), key(24));

        let server_t = ReticulumTransport::builder(&server_kp)
            .alpns(vec![alpn.clone()])
            .interfaces(vec![ReticulumInterface::TcpServer {
                bind: format!("127.0.0.1:{port}").parse().unwrap(),
            }])
            .announce_interval(Duration::from_millis(500))
            .bind()
            .await
            .expect("bind server");
        let client_t = ReticulumTransport::builder(&client_kp)
            .alpns(vec![alpn.clone()])
            .interfaces(vec![ReticulumInterface::TcpClient {
                addr: format!("127.0.0.1:{port}").parse().unwrap(),
            }])
            .announce_interval(Duration::from_millis(500))
            .connect_timeout(Duration::from_secs(20))
            .bind()
            .await
            .expect("bind client");
        let server_peer = server_t.local_peer_id();
        let (server_lane, dial_lane) = (lane(), lane());

        let server = async {
            let accepted =
                tokio::time::timeout(Duration::from_secs(25), server_t.accept(alpn.clone()))
                    .await
                    .expect("accept timed out")
                    .expect("accept");
            server_lane
                .serve_noise(
                    &server_key,
                    accepted.into_stream(),
                    |_| true,
                    right.session(),
                )
                .await
        };
        let dial = dial_lane.dial_noise(
            &client_t,
            server_peer,
            &dialer_key,
            id_of(&server_key),
            left.session(),
        );
        let (served, dialed) = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::join!(server, dial)
        })
        .await
        .expect("reticulum sync timed out");
        assert_eq!((done(dialed).accepted, done(served).accepted), (2, 3));
        converged(&left, &right).await;
    }
}

mod p2panda {
    use transport::P2pandaTransport;

    use super::*;

    // The client holds its FIN until the holder's session has returned. This is
    // the ordering hit by an empty reader catching up from a multi-edition log.
    struct FinishHolder(tokio::sync::oneshot::Sender<()>);

    impl StreamSession for FinishHolder {
        async fn run<S>(
            self,
            mut stream: S,
        ) -> Result<StreamSyncOutcome, murm_sync::StreamSyncError>
        where
            S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
        {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            assert_eq!(stream.read_u8().await.unwrap(), 1);
            stream.write_u8(2).await.unwrap();
            stream.shutdown().await.unwrap();
            self.0.send(()).unwrap();
            Ok(StreamSyncOutcome::default())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_holder_waits_for_the_readers_final_close() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let alpn = Alpn::new(PROTOCOL);
        let holder = P2pandaTransport::builder_from_seed_ref(&[36; 32])
            .alpns(vec![alpn.clone()])
            .bind()
            .await
            .unwrap();
        let reader = P2pandaTransport::builder_from_seed_ref(&[37; 32])
            .bind()
            .await
            .unwrap();
        reader
            .add_peer(holder.endpoint_addr().await.unwrap())
            .await
            .unwrap();
        let (finished, holder_finished) = tokio::sync::oneshot::channel();
        let serve = async {
            let accepted = holder.accept(alpn.clone()).await.unwrap();
            lane()
                .serve_native(accepted, |_| true, FinishHolder(finished))
                .await
                .unwrap();
        };
        let read = async {
            let mut stream = reader
                .connect(holder.local_peer_id(), alpn.clone())
                .await
                .unwrap();
            stream.write_u8(1).await.unwrap();
            assert_eq!(stream.read_u8().await.unwrap(), 2);
            holder_finished.await.unwrap();
            stream.shutdown().await.expect("reader FIN acknowledged");
        };
        tokio::time::timeout(Duration::from_secs(30), async {
            tokio::pin!(serve);
            tokio::select! {
                biased;
                () = &mut serve => panic!("holder released the connection before the reader closed"),
                () = read => {}
            }
            serve.await;
        }).await.expect("release sync close timed out");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn p2panda_peers_converge_through_native_security() {
        let (left, right) = halves().await;
        let alpn = Alpn::new(PROTOCOL);
        let build = |seed: u8| {
            let alpn = alpn.clone();
            async move {
                P2pandaTransport::builder_from_seed_ref(&[seed; 32])
                    .alpns(vec![alpn])
                    .bind()
                    .await
                    .expect("bind")
            }
        };
        let (client, server) = (build(31).await, build(32).await);
        let (client_addr, server_addr) = (
            client.endpoint_addr().await.unwrap(),
            server.endpoint_addr().await.unwrap(),
        );
        client.add_peer(server_addr).await.unwrap();
        server.add_peer(client_addr).await.unwrap();
        let server_peer = server.local_peer_id();
        let (server_lane, dial_lane) = (lane(), lane());

        let serving = async {
            let accepted = server.accept(alpn.clone()).await.unwrap();
            server_lane
                .serve_native(accepted, |_| true, right.session())
                .await
        };
        let dialing = dial_lane.dial_native(&client, server_peer, left.session());
        let (served, dialed) = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::join!(serving, dialing)
        })
        .await
        .expect("p2panda sync timed out");
        assert_eq!((done(dialed).accepted, done(served).accepted), (2, 3));
        converged(&left, &right).await;
    }
}
