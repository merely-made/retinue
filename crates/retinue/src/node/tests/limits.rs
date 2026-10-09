//! Logical MTU, payload limits, pause assessment and action bounds.

use super::*;

#[test]
fn logical_mtu_validates_configuration_and_announce_shapes() {
    let mut n = node();
    assert_eq!(n.logical_mtu(), LINK_MTU);
    assert!(!n.has_active_sessions());
    assert_eq!(
        n.set_logical_mtu(MIN_LOGICAL_MTU - 1),
        Err(LogicalMtuError::OutOfRange)
    );
    assert_eq!(
        n.set_logical_mtu(LINK_MTU + 1),
        Err(LogicalMtuError::OutOfRange)
    );
    n.set_logical_mtu(247).unwrap();
    n.try_set_app_data(&[1; 80]).unwrap();
    assert_eq!(
        n.try_announce(&blob([1; RAND_HASH_LEN]), None)
            .unwrap()
            .encoded_len(),
        247
    );
    assert!(
        n.try_announce(&blob([1; RAND_HASH_LEN]), Some(&[0; RATCHET_LEN]))
            .is_err()
    );
    assert!(n.try_set_app_data(&[2; 81]).is_err());
    assert_eq!(
        n.set_logical_mtu(246),
        Err(LogicalMtuError::AppDataTooLarge)
    );
    assert_eq!(n.logical_mtu(), 247);
}

#[test]
fn logical_mtu_negotiates_both_roles_and_bounds_direct_data() {
    let (mut a, mut b) = pair();
    a.set_logical_mtu(247).unwrap();
    b.set_logical_mtu(239).unwrap();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
    assert!(
        a.has_active_sessions(),
        "pending negotiation retains session state"
    );
    assert_eq!(a.set_logical_mtu(246), Err(LogicalMtuError::SessionsActive));
    let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
    let id = link_up(&a.ingest(IFACE, &proof, 0)).unwrap();
    assert!(a.has_active_sessions());
    assert!(b.has_active_sessions());
    assert_eq!(a.links[0].0.mtu(), 239);
    assert_eq!(b.links[0].0.mtu(), 239);
    assert_eq!(b.set_logical_mtu(238), Err(LogicalMtuError::SessionsActive));
    assert_eq!(b.set_logical_mtu(239), Ok(()));
    let exact = sent(
        &a.send(id, IFACE, &[0; 159], &[1; crate::token::IV_LEN])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(exact.encoded_len(), 227);
    assert!(
        a.send(id, IFACE, &[0; 160], &[2; crate::token::IV_LEN])
            .is_none()
    );
    assert!(
        b.send(id, IFACE, &[0; 160], &[3; crate::token::IV_LEN])
            .is_none()
    );
}

#[test]
fn logical_mtu_bridge_lifecycle_and_refused_request_leave_no_phantom_session() {
    let mut relay = node().with_transport_config(TransportConfig::transit());
    relay.remember_bridge(
        AddressHash::from_bytes([9; 16]),
        AddressHash::from_bytes([8; 16]),
        1,
        2,
        LINK_TRANSPORT_TIMEOUT,
        0,
    );
    assert!(relay.has_active_sessions());
    assert_eq!(
        relay.set_logical_mtu(247),
        Err(LogicalMtuError::SessionsActive)
    );
    relay.expire_transport_state(LINK_TRANSPORT_TIMEOUT);
    assert!(!relay.has_active_sessions());
    relay.set_logical_mtu(247).unwrap();
    let (mut source, destination) = pair();
    let announce = destination.announce(&blob([3; RAND_HASH_LEN]), None);
    relay.ingest(IFACE + 1, &announce, LINK_TRANSPORT_TIMEOUT);
    source.ingest(IFACE, &announce, 0);
    let mut request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x55; 64], 0)
            .unwrap(),
    )
    .unwrap();
    request.header_type = HeaderType::Type2;
    request.transport = Some(relay.identity.hash());
    request.payload.resize(250, 0);
    assert!(sent(&relay.ingest(IFACE, &request, LINK_TRANSPORT_TIMEOUT + 1)).is_none());
    assert!(relay.bridges.is_empty());
    assert_eq!(relay.refused_payloads(), 1);
    relay.set_logical_mtu(246).unwrap();
}

#[test]
fn relay_refuses_type_two_growth_beyond_logical_mtu() {
    let mut relay = node().with_transport_config(TransportConfig::transit());
    relay.set_logical_mtu(247).unwrap();
    let (_, mut peer) = pair();
    peer.try_set_app_data(&[0; 80]).unwrap();
    let packet = peer.announce(&blob([4; RAND_HASH_LEN]), None);
    assert_eq!(packet.encoded_len(), 247);
    let actions = relay.ingest(IFACE, &packet, 0);
    assert!(sent(&actions).is_none());
    assert_eq!(relay.refused_payloads(), 1);
    assert!(relay.peers().knows(peer.destination()));
}

#[test]
fn ingest_refuses_a_packet_carrying_the_ifac_flag() {
    let (mut node, peer) = pair();
    let mut flagged = peer.announce(&blob([5; RAND_HASH_LEN]), None);
    flagged.ifac = true;
    assert!(node.ingest(IFACE, &flagged, 0).is_empty());
    assert_eq!(node.refused_payloads(), 1);
    assert!(!node.peers().knows(peer.destination()));

    flagged.ifac = false;
    node.ingest(IFACE, &flagged, 0);
    assert_eq!(node.refused_payloads(), 1);
    assert!(node.peers().knows(peer.destination()));
}

#[test]
fn pause_assessment_rejects_a_clock_before_retained_link_activity() {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 1_000);
    let request = sent(
        &a.open_link(b.destination(), IFACE, &[0x31; 64], 1_000)
            .unwrap(),
    )
    .unwrap();
    let proof = sent(&b.ingest(IFACE, &request, 1_000)).unwrap();
    a.ingest(IFACE, &proof, 1_000);

    let assessment = a.pause_assessment();
    assert_eq!(assessment.latest_link_activity, Some(1_000));
    assert_eq!(
        assessment.can_pause_through(0, 100),
        Err(PauseBlocked::ClockBeforeLinkActivity {
            now: 0,
            latest_activity: 1_000,
        })
    );
}

/// Actions are bounded, and say so when they fill.
#[test]
fn actions_report_overflow_rather_than_dropping_silently() {
    let mut actions = Actions::<2>::new();
    for _ in 0..5 {
        actions.push(Action::Learned {
            destination: AddressHash::from_bytes([0; 16]),
        });
    }
    assert_eq!(actions.len(), 2, "held to its bound");
    assert_eq!(actions.overflowed(), 3, "and counted what did not fit");
}
