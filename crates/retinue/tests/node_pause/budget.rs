use super::*;

fn budget_node() -> TestNode {
    Node::new_with_payload_limits(
        PrivateIdentity::from_secret_bytes(&[0x22; 64]),
        DestinationName::new("retinue", ["capacity"]).name_hash(),
        retinue::node::PayloadLimits {
            max_ingress_bytes: 255,
            max_app_data: 8,
            max_link_payload: 32,
            max_outbound_resource: 350,
            max_resource_parts: 1,
        },
    )
}

#[test]
fn oversized_announce_is_refused_before_learning_or_freshness() {
    let mut node = budget_node();
    // Built directly: the local announce setter would refuse this payload.
    let packet = retinue::announce::build(
        &PrivateIdentity::from_secret_bytes(&[0x11; 64]),
        DestinationName::new("retinue", ["oversized"]).name_hash(),
        &retinue::announce::AnnounceBlob::from_wire([2; RAND_HASH_LEN]),
        None,
        &[0; 256],
    );
    assert!(node.ingest(IFACE, &packet, 0).is_empty());
    assert_eq!(node.refused_payloads(), 1);
    assert_eq!(node.peers().len(), 0);
    assert_eq!(node.route_count(), 0);
}

#[test]
fn refused_app_data_keeps_previous_announce() {
    let mut node = budget_node();
    node.try_set_app_data(b"12345678").unwrap();
    let blob = retinue::announce::AnnounceBlob::from_wire([2; RAND_HASH_LEN]);
    let before = node.announce(&blob, None).encode();
    assert_eq!(
        node.try_set_app_data(b"123456789"),
        Err(retinue::node::AppDataTooLarge)
    );
    assert_eq!(node.announce(&blob, None).encode(), before);
}

#[test]
fn payload_budgets_refuse_oversized_work_without_losing_the_link() {
    let mut small = budget_node();
    let mut peer = pair().0;
    let blob = retinue::announce::AnnounceBlob::from_wire([2; RAND_HASH_LEN]);
    peer.ingest(IFACE, &small.announce(&blob, None), 0);
    let request = sent(
        &peer
            .open_link(small.destination(), IFACE, &[0x31; 64], 0)
            .unwrap(),
    );
    let proof = sent(&small.ingest(IFACE, &request, 0));
    let id = link_up(&peer.ingest(IFACE, &proof, 0));
    assert!(small.send(id, IFACE, &[0; 33], &[4; 16]).is_none());
    assert!(small.send(id, IFACE, &[0; 32], &[5; 16]).is_some());
    assert!(
        small
            .publish(id, IFACE, &[0; 351], [6; 4], &[7; 16], 0)
            .is_none()
    );
    assert_eq!(small.refused_payloads(), 1);
    assert!(!small.transfer_active(id));
    assert!(
        small
            .publish(id, IFACE, &[0; 350], [6; 4], &[7; 16], 0)
            .is_some()
    );
    assert!(small.has_link(id));
}

#[test]
fn configured_resource_part_limit_refuses_offer() {
    let mut small = budget_node();
    let mut peer = pair().0;
    // Incompressible, so bz2 cannot shrink the offer under the one-part ceiling.
    let payload: Vec<u8> = (0..350u32)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 8) as u8)
        .collect();
    #[cfg(feature = "compression")]
    assert!(retinue::resource::compress(&payload).len() >= payload.len());
    let blob = retinue::announce::AnnounceBlob::from_wire([2; RAND_HASH_LEN]);
    peer.ingest(IFACE, &small.announce(&blob, None), 0);
    let request = sent(
        &peer
            .open_link(small.destination(), IFACE, &[0x31; 64], 0)
            .unwrap(),
    );
    let proof = sent(&small.ingest(IFACE, &request, 0));
    let id = link_up(&peer.ingest(IFACE, &proof, 0));
    let offer = sent(
        &peer
            .publish(id, IFACE, &payload, [6; 4], &[7; 16], 0)
            .unwrap(),
    );
    // The refusal goes on the wire, as RNS rejects an offer: one sealed receiver cancel.
    let refusal = sent(&small.ingest(IFACE, &offer, 0));
    assert_eq!(refusal.context, retinue::link::CTX_RESOURCE_RCL);
    assert_eq!(small.refused_offers(), 1);
    assert!(!small.transfer_active(id));
    assert!(small.has_link(id));
}
