//! Per-packet cost: work the node must not repeat or spend on a packet it will refuse.

use super::*;
use crate::probe::{Probe, take};

#[test]
fn a_link_proof_is_verified_once_among_pending_attempts() {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    a.open_link(b.destination(), IFACE, &[0x32; 64], 0).unwrap();
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
    let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();

    take(Probe::LinkProve);
    assert!(link_up(&a.ingest(IFACE, &proof, 0)).is_some());
    assert_eq!(take(Probe::LinkProve), 1);
}

#[test]
fn a_replayed_or_stale_announce_is_refused_unverified() {
    let (mut a, b) = pair();
    let mut timebase = [3; RAND_HASH_LEN];
    let announce = b.announce(&blob(timebase), None);
    take(Probe::AnnounceVerify);
    a.ingest(IFACE, &announce, 0);
    assert_eq!(take(Probe::AnnounceVerify), 1);

    let mut relayed = announce.clone();
    relayed.hops = 2;
    a.ingest(IFACE, &relayed, 1);
    timebase[RAND_HASH_LEN - 1] = 2;
    a.ingest(IFACE, &b.announce(&blob(timebase), None), 2);
    assert_eq!(take(Probe::AnnounceVerify), 0);
    assert_eq!(a.transport_counters.replayed_announces, 1);
    assert_eq!(a.transport_counters.stale_announces, 1);
}

#[test]
fn an_announce_for_another_destination_is_refused_unverified() {
    let (mut a, b) = pair();
    let mut forged = b.announce(&blob([3; RAND_HASH_LEN]), None);
    forged.destination = AddressHash::from_bytes([0x5A; 16]);
    take(Probe::AnnounceVerify);
    assert!(a.ingest(IFACE, &forged, 0).is_empty());
    assert_eq!(take(Probe::AnnounceVerify), 0);
    assert!(!a.peers().knows(forged.destination));
}

#[test]
fn link_data_is_hashed_once() {
    let (mut a, mut b, id) = linked();
    let data = sent(&b.send(id, IFACE, b"hello", &[0xA1; 16]).unwrap()).unwrap();
    take(Probe::PacketHash);
    a.ingest(IFACE, &data, 1);
    assert_eq!(take(Probe::PacketHash), 1);
    a.ingest(IFACE, &data, 2);
    assert_eq!(take(Probe::PacketHash), 1);
    assert_eq!(a.transport_counters.duplicate_dropped, 1);
}

#[test]
fn a_carried_packet_is_hashed_once() {
    let (_, destination) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    relay.ingest(
        IFACE + 1,
        &destination.announce(&blob([0x77; RAND_HASH_LEN]), None),
        0,
    );
    let packet = Packet {
        ifac: false,
        header_type: HeaderType::Type2,
        context_flag: false,
        propagation: crate::packet::Propagation::Transport,
        destination_type: crate::packet::DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport: Some(relay.identity.hash()),
        destination: destination.destination(),
        context: 0,
        payload: vec![1; 48],
    };
    take(Probe::PacketHash);
    assert!(sent(&relay.ingest(IFACE, &packet, 1)).is_some());
    assert_eq!(take(Probe::PacketHash), 1);
}
