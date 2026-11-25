// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! V7: the same owner policy, over real transports.
//!
//! The V6 suite proved the handshake over a duplex pair. This runs it through
//! actual `Transport` implementations, exercising the real N1 adapter
//! (`AcceptedSession::session_facts`) so the ingress facts come from the
//! carrier rather than a fixture.
//!
//! The Reticulum arm is behind the `reticulum` feature because it needs the
//! retinue stack; it is the arm that matters most, since Reticulum acceptance
//! is best-effort and the session proof is the *only* thing that names a
//! subject there.

use identity::delegation::Issue;
use std::collections::BTreeMap;

#[cfg(feature = "reticulum")]
use identity::Ed25519Keypair;
use identity::{IdentityProvider, InMemoryProvider};
use insigne::delegation::{
    CapabilityScope, DelegationCertificate, DelegationParent, SignedDelegationCertificate,
};
use notochord::{
    DenyReason, LocalNetworkPolicy, NetworkId, ProfileRef, ProofBinding, RequestedAction,
    RevocationLedger, ServiceAccess, ServiceRule, SessionDecision, SessionHello, TrafficClass,
    TrustedRoot, accept_session, initiate_session,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use transport::memory::MemoryTransport;
use transport::{Alpn, PeerID, Transport, initiator_binding};

const NETWORK: NetworkId = NetworkId([3; 32]);
const ROOT_AUTHORITY: [u8; 32] = [7; 32];
const MURM: &str = "/services/murm";
const NOW_MS: u64 = 50;
const APPLICATION_BYTES: &[u8] = b"murm-application-payload";

fn root() -> InMemoryProvider {
    InMemoryProvider::from_seed([1; 32])
}

/// The connecting member. Its transport identity and its personae identity are
/// the same key, which is what D6 requires on an authenticating transport.
fn member() -> InMemoryProvider {
    InMemoryProvider::from_seed([4; 32])
}

fn grant_to(subject: [u8; 32]) -> SignedDelegationCertificate {
    SignedDelegationCertificate::issue(
        &root(),
        DelegationCertificate::new(
            DelegationParent::Root(ROOT_AUTHORITY),
            root().master_public_key().to_bytes(),
            subject,
            CapabilityScope {
                domain: "mere.network".into(),
                resource: NETWORK.0.to_vec(),
                path_prefix: MURM.into(),
                actions: ["connect".to_string()].into_iter().collect(),
            },
            5,
            10,
            Some(100),
            1,
            [1; 32],
        ),
    )
    .expect("issue certificate")
}

fn policy(access: ServiceAccess) -> LocalNetworkPolicy {
    let mut policy = LocalNetworkPolicy::closed(NETWORK);
    policy.accepted_profiles = vec![ProfileRef {
        id: "mere.base".into(),
        revision: 1,
    }];
    policy.trusted_roots = vec![TrustedRoot {
        authority: ROOT_AUTHORITY,
        issuer: root().master_public_key().to_bytes(),
    }];
    policy.services = BTreeMap::from([(
        MURM.to_string(),
        ServiceRule::new(access, "mere.network", ["connect"], false, None),
    )]);
    policy
}

fn hello_for(binding: &ProofBinding) -> SessionHello {
    let subject = member().master_public_key().to_bytes();
    SessionHello::issue(
        &member(),
        NETWORK,
        ProfileRef {
            id: "mere.base".into(),
            revision: 2,
        },
        RequestedAction {
            domain: "mere.network".into(),
            path: MURM.into(),
            action: "connect".into(),
        },
        TrafficClass::Interactive,
        [42; 32],
        binding,
        vec![grant_to(subject)],
    )
    .expect("issue hello")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_transport_admits_and_refuses_by_owner_rule() {
    for (access, expect_accept) in [
        (ServiceAccess::MemberOnly, true),
        (ServiceAccess::Disabled, false),
    ] {
        let alpn = Alpn::new("mere/murm/v1");
        let member_peer = PeerID::from_public_key(member().master_public_key());
        let server_peer =
            PeerID::from_public_key(InMemoryProvider::from_seed([9; 32]).master_public_key());
        let (client_side, server_side) = MemoryTransport::pair(member_peer, server_peer);

        let policy = policy(access);
        let server_alpn = alpn.clone();
        let server_policy = policy.clone();
        let responder = tokio::spawn(async move {
            let mut session = server_side
                .accept(server_alpn)
                .await
                .expect("accept a session");
            let facts = session.session_facts();
            let decision = accept_session(
                &mut session.stream,
                &server_policy,
                &RevocationLedger::new(),
                &facts,
                NOW_MS,
                0,
            )
            .await
            .expect("responder handshake");
            if decision.is_accept() {
                session
                    .stream
                    .write_all(APPLICATION_BYTES)
                    .await
                    .expect("service write");
                session.stream.flush().await.expect("flush");
            }
            drop(session);
            decision
        });

        let mut stream = client_side
            .connect(server_peer, alpn.clone())
            .await
            .expect("connect");
        // The initiator knows the same facts: this transport authenticates, so
        // the peer it will be seen as is its own id.
        let binding = initiator_binding(&alpn, member_peer);
        let reply = initiate_session(&mut stream, &hello_for(&binding), &policy.limits)
            .await
            .expect("initiator handshake");

        let mut application = Vec::new();
        stream
            .read_to_end(&mut application)
            .await
            .expect("read application bytes");
        let decision = responder.await.expect("responder task");

        assert_eq!(decision.is_accept(), expect_accept);
        assert_eq!(reply.is_accept(), expect_accept, "both ends agree");
        if expect_accept {
            assert_eq!(application, APPLICATION_BYTES);
        } else {
            assert!(
                application.is_empty(),
                "a refused session delivers no application bytes"
            );
            match decision {
                SessionDecision::Deny { reason } => {
                    assert_eq!(reason, DenyReason::ServiceNotOffered)
                },
                SessionDecision::Accept { .. } => unreachable!(),
            }
        }
    }
}

#[cfg(feature = "reticulum")]
mod reticulum {
    use super::*;
    use std::time::Duration;
    use transport::{ReticulumInterface, ReticulumTransport};

    fn iface_server(port: u16) -> ReticulumInterface {
        ReticulumInterface::TcpServer {
            bind: format!("127.0.0.1:{port}").parse().expect("bind addr"),
        }
    }

    fn iface_client(port: u16) -> ReticulumInterface {
        ReticulumInterface::TcpClient {
            addr: format!("127.0.0.1:{port}").parse().expect("peer addr"),
        }
    }

    #[cfg(feature = "noise")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reticulum_tcp_noise_admits_a_secure_subject_on_the_real_shared_link() {
        tokio::time::timeout(Duration::from_secs(45), async {
            use notochord::{SecureIdentityPolicy, admit_session_secure};
            use transport::noise::{secure_initiator_with_facts, secure_responder_with_facts};

            let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            drop(socket);
            let alpn = Alpn::new("mere/murm/v1");
            let server_carrier = Ed25519Keypair::from_seed([71; 32]);
            let client_carrier = Ed25519Keypair::from_seed([72; 32]);
            let server_peer = PeerID::from_public_key(server_carrier.public_key());
            let server_noise = Ed25519Keypair::from_seed([73; 32]);
            let expected_server_noise = PeerID::from_public_key(server_noise.public_key());
            let client_noise = Ed25519Keypair::from_seed([4; 32]);
            let expected_subject = member().master_public_key().to_bytes();

            let server = ReticulumTransport::builder(&server_carrier)
                .network_stamps(transport::NetworkStampPolicy::rns_1_6_0())
                .alpns(vec![alpn.clone()])
                .interfaces(vec![iface_server(port)])
                .announce_interval(Duration::from_millis(200))
                .reliable_links(true)
                .bind()
                .await
                .unwrap();
            let client = ReticulumTransport::builder(&client_carrier)
                .network_stamps(transport::NetworkStampPolicy::rns_1_6_0())
                .alpns(vec![alpn.clone()])
                .interfaces(vec![iface_client(port)])
                .announce_interval(Duration::from_millis(200))
                .reliable_links(true)
                .bind()
                .await
                .unwrap();
            let (read_done_tx, read_done) = tokio::sync::oneshot::channel::<()>();
            let server_alpn = alpn.clone();
            let responder = tokio::spawn(async move {
                let accepted = server.accept(server_alpn).await.unwrap();
                assert_eq!(accepted.peer, None);
                let carrier = accepted.session_facts();
                let original = carrier.clone();
                let (stream, noise) = secure_responder_with_facts(&server_noise, accepted.stream)
                    .await
                    .unwrap();
                assert_eq!(noise.peer().to_bytes(), expected_subject);
                let secure_channel = noise.secure_channel_facts();
                let mut admitted = admit_session_secure(
                    stream,
                    &policy(ServiceAccess::MemberOnly),
                    &RevocationLedger::new(),
                    &carrier,
                    &secure_channel,
                    SecureIdentityPolicy::MatchCarrierWhenPresent,
                    NOW_MS,
                    0,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(admitted.session.facts, original);
                assert_eq!(admitted.session.facts.authenticated_initiator, None);
                assert_eq!(admitted.session.principal.subject, expected_subject);
                admitted
                    .session
                    .stream
                    .write_all(APPLICATION_BYTES)
                    .await
                    .unwrap();
                admitted.session.stream.flush().await.unwrap();
                let _ = read_done.await;
                (carrier, secure_channel)
            });

            let outbound = client
                .connect_session(server_peer, alpn.clone())
                .await
                .unwrap();
            assert_eq!(outbound.authenticated_initiator, None);
            assert_eq!(outbound.shared_link, Some(outbound.stream.link_id()));
            let carrier_binding = outbound.proof_binding();
            let (mut stream, noise) =
                secure_initiator_with_facts(&client_noise, outbound.stream, &alpn)
                    .await
                    .unwrap();
            assert_eq!(noise.peer(), expected_server_noise);
            let secure_channel = noise.secure_channel_facts();
            let binding = carrier_binding.with_secure_channel(secure_channel.clone());
            let hello = SessionHello::issue_secure(
                &member(),
                NETWORK,
                ProfileRef {
                    id: "mere.base".into(),
                    revision: 1,
                },
                RequestedAction {
                    domain: "mere.network".into(),
                    path: MURM.into(),
                    action: "connect".into(),
                },
                TrafficClass::Interactive,
                [81; 32],
                &binding,
                vec![grant_to(expected_subject)],
            )
            .unwrap();
            let reply = initiate_session(
                &mut stream,
                &hello,
                &policy(ServiceAccess::MemberOnly).limits,
            )
            .await
            .unwrap();
            assert!(reply.is_accept());
            let mut application = vec![0; APPLICATION_BYTES.len()];
            stream.read_exact(&mut application).await.unwrap();
            assert_eq!(application, APPLICATION_BYTES);
            let _ = read_done_tx.send(());
            let (server_carrier, server_secure) = responder.await.unwrap();
            assert_eq!(server_carrier.proof_binding(), binding.carrier);
            assert_eq!(server_secure, secure_channel);
        })
        .await
        .expect("Reticulum/Noise/v2 exchange must finish within its fixture deadline");
    }

    /// The Reticulum arm. Acceptance here is best-effort: the transport cannot
    /// name the initiator, so `peer` is `None` and the session proof is the
    /// only thing that establishes a subject. The transcript still binds the
    /// interface and link the session actually arrived on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reticulum_tcp_admits_by_session_proof_alone() {
        let alpn = Alpn::new("mere/murm/v1");
        let port = 47_833u16;
        let server_kp = Ed25519Keypair::from_seed([21u8; 32]);
        let client_kp = Ed25519Keypair::from_seed([22u8; 32]);
        let server_peer = PeerID::from_public_key(server_kp.public_key());

        let server = ReticulumTransport::builder(&server_kp)
            .network_stamps(transport::NetworkStampPolicy::rns_1_6_0())
            .alpns(vec![alpn.clone()])
            .interfaces(vec![iface_server(port)])
            .announce_interval(Duration::from_millis(500))
            .bind()
            .await
            .expect("bind server");
        let client = ReticulumTransport::builder(&client_kp)
            .network_stamps(transport::NetworkStampPolicy::rns_1_6_0())
            .alpns(vec![alpn.clone()])
            .interfaces(vec![iface_client(port)])
            .announce_interval(Duration::from_millis(500))
            .connect_timeout(Duration::from_secs(20))
            .bind()
            .await
            .expect("bind client");

        let server_alpn = alpn.clone();
        let (read_done_tx, read_done) = tokio::sync::oneshot::channel::<()>();
        let responder = tokio::spawn(async move {
            let mut session =
                tokio::time::timeout(Duration::from_secs(25), server.accept(server_alpn))
                    .await
                    .expect("accept timed out")
                    .expect("accept failed");
            assert_eq!(
                session.peer, None,
                "reticulum acceptance cannot name its initiator"
            );
            let facts = session.session_facts();
            assert!(
                facts.ingress.shared_link.is_some(),
                "the transcript must have a link to bind to"
            );
            let decision = accept_session(
                &mut session.stream,
                &policy(ServiceAccess::MemberOnly),
                &RevocationLedger::new(),
                &facts,
                NOW_MS,
                0,
            )
            .await
            .expect("responder handshake");
            if decision.is_accept() {
                session
                    .stream
                    .write_all(APPLICATION_BYTES)
                    .await
                    .expect("service write");
                session.stream.flush().await.expect("flush");
            }
            // Dropping the stream here is safe: the outbound relay drains the
            // duplex before it closes the link. What is *not* safe is dropping
            // the transport, which tears the endpoint down and aborts those
            // relays mid-flight, discarding bytes `flush` already reported as
            // written. `server` is owned by this task, so returning early
            // would do exactly that. A real service outlives its sessions;
            // holding here until the client has read models that.
            drop(session);
            let _ = read_done.await;
            (decision, facts)
        });

        let outbound = tokio::time::timeout(
            Duration::from_secs(25),
            client.connect_session(server_peer, alpn.clone()),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        // Carrier metadata reports the real shared link, never a guessed local
        // identity. It must agree with both the stream and responder facts.
        let binding = outbound.proof_binding();
        assert_eq!(outbound.authenticated_initiator, None);
        assert_eq!(outbound.shared_link, Some(outbound.stream.link_id()));
        let stream_link = outbound.stream.link_id();
        let mut stream = outbound.stream;
        let reply = tokio::time::timeout(
            Duration::from_secs(30),
            initiate_session(
                &mut stream,
                &hello_for(&binding),
                &policy(ServiceAccess::MemberOnly).limits,
            ),
        )
        .await
        .expect("handshake timed out: no reply from the responder")
        .expect("initiator handshake");

        // read_exact, not read_to_end: a retinue link does not necessarily
        // signal EOF promptly when the far side drops it, and waiting for a
        // close we never arranged is what hung this test the first time.
        let mut application = vec![0u8; APPLICATION_BYTES.len()];
        tokio::time::timeout(Duration::from_secs(30), stream.read_exact(&mut application))
            .await
            .expect("application read timed out")
            .expect("read application bytes");
        // Release the responder now that everything it sent has been read.
        let _ = read_done_tx.send(());

        let (decision, server_facts) = tokio::time::timeout(Duration::from_secs(30), responder)
            .await
            .expect("responder task timed out")
            .expect("responder task");
        assert!(
            decision.is_accept(),
            "a member proof admits over best-effort Reticulum acceptance"
        );
        assert!(reply.is_accept(), "both ends agree");
        assert_eq!(
            server_facts.ingress.shared_link,
            Some(stream_link),
            "both ends bound the same link id"
        );
        assert_eq!(
            application, APPLICATION_BYTES,
            "the service speaks only after admission"
        );
    }
}
