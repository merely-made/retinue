// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! Bilateral-lane tests for [`ReticulumTransport`] over TCP loopback.

use std::time::Duration;

use identity::Ed25519Keypair;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{ReticulumInterface, ReticulumTransport};
use crate::{Alpn, PeerID, Transport, TransportKind};

/// A distinct-per-test loopback port. Reticulum's TCP interface takes a string
/// address and does not expose an ephemeral-port readback, so tests pin a port.
fn iface_server(port: u16) -> ReticulumInterface {
    ReticulumInterface::TcpServer {
        bind: format!("127.0.0.1:{port}").parse().unwrap(),
    }
}

fn iface_client(port: u16) -> ReticulumInterface {
    ReticulumInterface::TcpClient {
        addr: format!("127.0.0.1:{port}").parse().unwrap(),
    }
}

#[tokio::test]
async fn derived_identity_is_stable_and_peer_id_matches() {
    let alpn = Alpn::new("mere/cable/v1");
    let kp = Ed25519Keypair::from_seed([7u8; 32]);
    let expected = PeerID::from_public_key(kp.public_key());

    let a = ReticulumTransport::builder(&kp)
        .network_stamps(crate::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![alpn.clone()])
        .bind()
        .await
        .expect("bind a");
    let b = ReticulumTransport::builder(&kp)
        .network_stamps(crate::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![alpn])
        .bind()
        .await
        .expect("bind b");

    // PeerID is the master public key, independent of the transport.
    assert_eq!(a.local_peer_id(), expected);
    // Same master seed => same Reticulum destination address across instances.
    assert_eq!(a.local_peer_id(), b.local_peer_id());
}

#[test]
fn unassociated_identity_shares_no_half_with_the_persona() {
    use identity::{IdentityProvider, InMemoryProvider, UnassociatedKeypair};

    let provider = InMemoryProvider::from_seed([10u8; 32]);
    let persona = super::keys::derive_identity(&provider.derived_keypair(b"anon").unwrap());
    let master = super::keys::derive_identity(provider.master_keypair());
    let key = UnassociatedKeypair::generate();
    let anon = super::keys::derive_identity(&key);

    for known in [&persona, &master] {
        let (a, k) = (
            anon.public().to_public_bytes(),
            known.public().to_public_bytes(),
        );
        assert_ne!(a[..32], k[..32], "x25519 half");
        assert_ne!(a[32..], k[32..], "ed25519 half");
    }
    assert_eq!(anon.public().ed25519_bytes(), &key.public_key().to_bytes());
}

#[tokio::test]
async fn announce_identity_is_the_peer_and_fits_a_direct_phy_frame() {
    let alpn = Alpn::new("mere/murm/v1");
    let kp = Ed25519Keypair::from_seed([8u8; 32]);
    let derived = super::keys::derive_identity(&kp);
    assert_eq!(
        derived.public().ed25519_bytes(),
        &kp.public_key().to_bytes(),
        "the verified Reticulum signing key is Mere's PeerID"
    );

    // Stamped, as RNS 1.6.1 emits by default: the extension must fit the frame too.
    let stamped = crate::NetworkStampPolicy {
        destination_cost: crate::StampCost::from_integer(8),
        destination_value: crate::StampCost::from_integer(8),
        ..Default::default()
    };
    let transport = ReticulumTransport::builder(&kp)
        .alpns(vec![alpn])
        .announce_interval(Duration::from_secs(60))
        .network_stamps(stamped)
        .bind()
        .await
        .expect("bind");
    let mut interface = transport.attach_packet_interface();
    transport.announce_now();
    let packet = tokio::time::timeout(Duration::from_secs(1), interface.next_outbound())
        .await
        .expect("announce timed out")
        .expect("endpoint closed");
    let announce = retinue::announce::Announce::decode(&packet).expect("an announce");
    assert!(
        announce.stamp_valid(stamped.destination_cost),
        "the announce is stamped"
    );
    let encoded_len = packet.encode().len();
    assert!(
        encoded_len <= 255,
        "authenticated announce is {encoded_len} bytes, over the direct-PHY frame cap"
    );
}

#[tokio::test]
async fn accept_unregistered_alpn_errors() {
    let kp = Ed25519Keypair::from_seed([9u8; 32]);
    let t = ReticulumTransport::builder(&kp)
        .network_stamps(crate::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![Alpn::new("mere/cable/v1")])
        .bind()
        .await
        .expect("bind");

    let err = t.accept(Alpn::new("mere/coop/v1")).await.unwrap_err();
    assert!(matches!(err, crate::TransportError::AlpnNotRegistered));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bilateral_round_trip_over_tcp_loopback() {
    let alpn = Alpn::new("mere/cable/v1");
    let port = 47_811u16;

    let server_kp = Ed25519Keypair::from_seed([1u8; 32]);
    let client_kp = Ed25519Keypair::from_seed([2u8; 32]);
    let server_peer = PeerID::from_public_key(server_kp.public_key());

    let server = ReticulumTransport::builder(&server_kp)
        .network_stamps(crate::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![alpn.clone()])
        .interfaces(vec![iface_server(port)])
        .announce_interval(Duration::from_millis(500))
        .bind()
        .await
        .expect("bind server");

    let client = ReticulumTransport::builder(&client_kp)
        .network_stamps(crate::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![alpn.clone()])
        .interfaces(vec![iface_client(port)])
        .announce_interval(Duration::from_millis(500))
        .connect_timeout(Duration::from_secs(20))
        .bind()
        .await
        .expect("bind client");

    // Server accepts an inbound link and echoes a reply.
    let alpn_server = alpn.clone();
    let accept = tokio::spawn(async move {
        let accepted =
            tokio::time::timeout(Duration::from_secs(25), server.accept(alpn_server.clone()))
                .await
                .expect("accept timed out")
                .expect("accept failed");

        // Best-effort Reticulum acceptance cannot identify its initiator, so
        // the honest answer is `None` — an application identity arrives later
        // through a session proof (plan D4/D6). Ingress is still a fact: the
        // interface and link the session actually arrived on.
        assert!(
            !accepted.is_transport_authenticated(),
            "reticulum best-effort must not claim an authenticated peer"
        );
        assert_eq!(accepted.peer, None);
        assert_eq!(accepted.protocol, alpn_server);
        assert_eq!(accepted.ingress.transport, TransportKind::Reticulum);
        assert!(
            accepted.ingress.interface.is_some(),
            "the interface the link arrived on must survive accept"
        );
        assert!(
            accepted.ingress.link.is_some(),
            "the link identity must survive accept"
        );

        let mut stream = accepted.into_stream();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"world").await.expect("server write");
        stream.flush().await.expect("server flush");
        // Keep the server (and its link) alive long enough for the reply to land.
        tokio::time::sleep(Duration::from_millis(500)).await;
    });

    // Client connects to the server and exchanges one message each way.
    let mut stream = client
        .connect(server_peer, alpn.clone())
        .await
        .expect("client connect");
    stream.write_all(b"hello").await.expect("client write");
    stream.flush().await.expect("client flush");

    let mut reply = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(15), stream.read_exact(&mut reply))
        .await
        .expect("client read timed out")
        .expect("client read");
    assert_eq!(&reply, b"world");

    accept.await.expect("accept task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bilateral_round_trip_over_attached_packet_interfaces() {
    let alpn = Alpn::new("mere/cable/v1");

    let server_kp = Ed25519Keypair::from_seed([3u8; 32]);
    let client_kp = Ed25519Keypair::from_seed([4u8; 32]);
    let server_peer = PeerID::from_public_key(server_kp.public_key());

    let server = ReticulumTransport::builder(&server_kp)
        .network_stamps(crate::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![alpn.clone()])
        .announce_interval(Duration::from_secs(60))
        .link_mtu(255)
        .reliable_links(true)
        .reliable_initial_rtt(Duration::from_millis(50))
        .reliable_max_window(1)
        .bind()
        .await
        .expect("bind server");

    let client = ReticulumTransport::builder(&client_kp)
        .network_stamps(crate::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![alpn.clone()])
        .announce_interval(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .link_mtu(255)
        .reliable_links(true)
        .reliable_initial_rtt(Duration::from_millis(50))
        .reliable_max_window(1)
        .bind()
        .await
        .expect("bind client");

    let server_interface = server.attach_packet_interface();
    let client_interface = client.attach_packet_interface();
    let (mut server_outbound, server_sink) = server_interface.split();
    let (mut client_outbound, client_sink) = client_interface.split();

    // This is the same packet seam a Tulle radio driver owns. Keeping the
    // bridge deterministic proves ReticulumTransport does not depend on TCP
    // before a serial port and real RF enter the receipt.
    let bridge = tokio::spawn(async move {
        loop {
            tokio::select! {
                packet = server_outbound.recv() => match packet {
                    Some(packet) => assert!(
                        client_sink.deliver(packet),
                        "client endpoint closed while bridge was active"
                    ),
                    None => break,
                },
                packet = client_outbound.recv() => match packet {
                    Some(packet) => assert!(
                        server_sink.deliver(packet),
                        "server endpoint closed while bridge was active"
                    ),
                    None => break,
                },
            }
        }
    });
    server.announce_now();

    let alpn_server = alpn.clone();
    let accept = tokio::spawn(async move {
        let accepted =
            tokio::time::timeout(Duration::from_secs(15), server.accept(alpn_server.clone()))
                .await
                .expect("accept timed out")
                .expect("accept failed");

        assert_eq!(accepted.protocol, alpn_server);
        assert_eq!(accepted.peer, None);
        assert_eq!(accepted.ingress.transport, TransportKind::Reticulum);
        assert!(accepted.ingress.interface.is_some());
        assert!(accepted.ingress.link.is_some());

        let mut stream = accepted.into_stream();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"world").await.expect("server write");
        stream.flush().await.expect("server flush");
        tokio::time::sleep(Duration::from_millis(250)).await;
    });

    let mut stream = client
        .connect(server_peer, alpn)
        .await
        .expect("client connect");
    stream.write_all(b"hello").await.expect("client write");
    stream.flush().await.expect("client flush");

    let mut reply = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut reply))
        .await
        .expect("client read timed out")
        .expect("client read");
    assert_eq!(&reply, b"world");

    accept.await.expect("accept task");
    bridge.abort();
}

/// Path segments become separate aspects, and the expanded name (which the destination
/// hash is taken over) is the same dotted string the single-aspect mapping produced.
#[test]
fn alpn_segments_map_to_aspects_with_the_same_expanded_name() {
    let name = super::destination_name_for_alpn(&Alpn::new("mere/cable/v1"));
    assert_eq!(name.expanded(), "mere.cable.v1");
    let name = super::destination_name_for_alpn(&Alpn::new("mere/murm/v1"));
    assert_eq!(name.expanded(), "mere.murm.v1");
}
