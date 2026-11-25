// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! An unassociated key (release log D3) binds production transports, and
//! nothing it yields matches the persona's derived or root keys.

use identity::{IdentityProvider, InMemoryProvider, UnassociatedKeypair};
use transport::p2panda_transport::P2pandaTransport;
use transport::{Alpn, PeerID, Transport};

fn persona_peers(provider: &InMemoryProvider) -> [PeerID; 2] {
    [
        PeerID::from_public_key(provider.master_public_key()),
        PeerID::from_public_key(provider.derived_keypair(b"anon").unwrap().public_key()),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unassociated_key_binds_p2panda_and_links_to_no_persona() {
    let provider = InMemoryProvider::from_seed([61; 32]);
    let key = UnassociatedKeypair::generate();
    let transport = P2pandaTransport::builder_for(&key)
        .alpns(vec![Alpn::new("mere/test/v1")])
        .bind()
        .await
        .expect("bind from an unassociated key");

    assert_eq!(
        transport.local_peer_id(),
        PeerID::from_public_key(key.public_key())
    );
    assert!(!persona_peers(&provider).contains(&transport.local_peer_id()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn derived_key_still_binds_p2panda() {
    let provider = InMemoryProvider::from_seed([62; 32]);
    let derived = provider.derived_keypair(b"anon").unwrap();
    let transport = P2pandaTransport::builder_for(&derived)
        .alpns(vec![Alpn::new("mere/test/v1")])
        .bind()
        .await
        .expect("bind from a derived key");
    assert_eq!(transport.local_peer_id(), persona_peers(&provider)[1]);
}

#[cfg(feature = "reticulum")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unassociated_key_binds_reticulum_with_its_own_peer() {
    use transport::reticulum_transport::ReticulumTransport;

    let provider = InMemoryProvider::from_seed([63; 32]);
    let key = UnassociatedKeypair::generate();
    let transport = ReticulumTransport::builder_for(&key)
        .network_stamps(transport::NetworkStampPolicy::rns_1_6_0())
        .alpns(vec![Alpn::new("mere/test/v1")])
        .bind()
        .await
        .expect("bind from an unassociated key");
    assert_eq!(
        transport.local_peer_id(),
        PeerID::from_public_key(key.public_key())
    );
    assert!(!persona_peers(&provider).contains(&transport.local_peer_id()));
}
