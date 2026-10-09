use alloc::vec;

use super::*;
use crate::link::CTX_CACHE_REQUEST;
use crate::resource::parse_proof;

/// A publisher that has sent every part and heard no proof asks for it with a cache
/// request naming the expected proof packet's full hash, at most three times; the
/// receiver's kept proof is exactly that packet.
#[test]
fn a_lost_proof_is_asked_for_by_hash() {
    let mut ivg = iv_gen();
    let (mut sender, receiver, proof) = transfer_until_proof(&payload(3000), &mut ivg);
    assert!(sender.awaiting_proof());
    let request = sender.cache_request().expect("a cache request");
    assert_eq!(request.context, CTX_CACHE_REQUEST);
    assert_eq!(request.payload, proof.full_hash());
    assert_eq!(receiver.proof_packet(), Some(proof.clone()));
    assert!(sender.cache_request().is_some());
    assert!(sender.cache_request().is_some());
    assert!(sender.cache_request().is_none(), "three at most");
    sender.on_packet(&proof, &mut ivg);
    assert!(sender.is_done());
    assert!(!sender.awaiting_proof());
}

/// A receiver that already proved a resource answers a re-sent offer of it with the
/// proof again: the sender lost it.
#[test]
fn a_re_advertisement_of_a_proved_resource_is_answered_with_the_proof() {
    let mut ivg = iv_gen();
    let (sender, mut receiver, proof) = transfer_until_proof(&payload(3000), &mut ivg);
    let answer = receiver.on_packet(&sender.advertisement(&ivg()), &mut ivg);
    assert_eq!(answer, vec![proof]);
}

/// A proof that names another resource does not complete the publisher.
#[test]
fn a_proof_for_another_resource_does_not_complete() {
    let mut ivg = iv_gen();
    let (mut sender, _, proof) = transfer_until_proof(&payload(3000), &mut ivg);
    let mut forged = proof.clone();
    forged.payload[0] ^= 1;
    sender.on_packet(&forged, &mut ivg);
    assert!(!sender.is_done());
    sender.on_packet(&proof, &mut ivg);
    assert!(sender.is_done());
}

/// RNS concludes a resource only on a PROOF-type packet (`Link.receive` dispatches
/// RESOURCE_PRF under `PacketType::Proof`), so the receipt and its retransmissions must
/// be one, unencrypted, carrying `resource_hash || proof`.
#[test]
fn the_resource_proof_is_a_proof_type_packet() {
    let mut ivg = iv_gen();
    let data = payload(3000);
    let (mut sender, mut receiver, proof) = transfer_until_proof(&data, &mut ivg);
    assert_eq!(proof.packet_type, crate::packet::PacketType::Proof);
    let (hash, _) = parse_proof(&proof.payload).expect("hash || proof, in the clear");
    assert_eq!(hash, sender.out.resource_hash());

    let replayed = receiver.retransmit(&mut ivg);
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].packet_type, crate::packet::PacketType::Proof);
    assert_eq!(replayed[0].payload, proof.payload);

    sender.on_packet(&proof, &mut ivg);
    assert!(
        sender.is_done(),
        "the PROOF-type receipt completes the sender"
    );
}

/// For one release a sender still accepts the DATA-type proof older retinue sent.
#[test]
fn a_sender_still_accepts_the_legacy_data_type_proof() {
    let mut ivg = iv_gen();
    let data = payload(1200);
    let (mut sender, _, proof) = transfer_until_proof(&data, &mut ivg);
    let legacy = sender
        .link
        .framed_packet(CTX_RESOURCE_PRF, proof.payload.clone());
    assert_eq!(legacy.packet_type, crate::packet::PacketType::Data);
    sender.on_packet(&legacy, &mut ivg);
    assert!(sender.is_done());
}
