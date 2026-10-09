//! The Retinue carrier: protected ingress, physical limits, and startup MTU budgets.

use super::*;

#[test]
fn protected_retinue_runtime_authenticates_before_protocol_ingress() {
    use radio_hand::retinue_carrier::RetinueCarrier;
    use retinue::ifac::Ifac;
    let ifac = Ifac::new(Some("test-carrier"), Some("correct"), 8).unwrap();
    let mut runtime = runtime_with_carrier(2, 10, RetinueCarrier::protected(ifac.clone()));
    assert_eq!(runtime.retinue().node().logical_mtu(), 247);
    let peer = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[9; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let blob = retinue::announce::AnnounceBlob::mint([1; 5], 1).unwrap();
    let packet = peer.announce(&blob, None);
    let logical = packet.encode();
    assert!(matches!(
        runtime.ingest(0, &logical),
        Err(Error::Carrier(retinue::Error::BadIfac))
    ));
    let wrong = Ifac::new(Some("test-carrier"), Some("wrong"), 8)
        .unwrap()
        .seal(&logical)
        .unwrap();
    assert!(matches!(
        runtime.ingest(0, &wrong),
        Err(Error::Carrier(retinue::Error::BadIfac))
    ));
    let mut damaged = ifac.seal(&logical).unwrap();
    *damaged.last_mut().unwrap() ^= 1;
    assert!(matches!(
        runtime.ingest(0, &damaged),
        Err(Error::Carrier(retinue::Error::BadIfac))
    ));
    assert!(
        runtime
            .retinue()
            .node()
            .peers()
            .resolve(packet.destination)
            .is_none()
    );
    assert_eq!(runtime.carrier_rejections(), (3, 0));
    runtime.ingest(0, &ifac.seal(&logical).unwrap()).unwrap();
    assert!(
        runtime
            .retinue()
            .node()
            .peers()
            .resolve(packet.destination)
            .is_some()
    );
    runtime.poll(0, Some(&blob)).unwrap();
    let tx = runtime.begin_tx(0).unwrap().unwrap();
    assert!(tx.frame.len() <= 255);
    let outgoing = ifac.open(&tx.frame).unwrap();
    let announce = retinue::Packet::decode(&outgoing).unwrap();
    assert_eq!(announce.destination, runtime.retinue().node().destination());
    runtime.complete_tx(1, tx.id, true).unwrap();
}

#[test]
fn carrier_checks_exact_physical_limit_and_relay_growth() {
    use radio_hand::retinue_carrier::RetinueCarrier;
    use retinue::{ifac::Ifac, packet::HeaderType};
    let carrier = RetinueCarrier::protected(Ifac::new(Some("limit"), None, 8).unwrap());
    let runtime = runtime(2, 10);
    let mut packet = runtime.retinue().node().announce(
        &retinue::announce::AnnounceBlob::mint([1; 5], 1).unwrap(),
        None,
    );
    packet
        .payload
        .resize(247 - retinue::packet::HEADER_MIN_LEN, 0);
    let wire = carrier.encode(&packet).unwrap();
    assert_eq!(wire.len(), 255);
    assert_eq!(carrier.decode(&wire).unwrap().encode(), packet.encode());
    packet.header_type = HeaderType::Type2;
    packet.transport = Some(packet.destination);
    assert!(matches!(
        carrier.encode(&packet),
        Err(retinue::Error::Oversize)
    ));
    packet.header_type = HeaderType::Type1;
    packet.transport = None;
    packet.payload.push(0);
    assert!(matches!(
        carrier.encode(&packet),
        Err(retinue::Error::Oversize)
    ));
    assert_eq!(
        RetinueCarrier::default().encode(&packet).unwrap(),
        packet.encode()
    );
}

#[test]
fn startup_carrier_preserves_stricter_node_budget() {
    use radio_hand::retinue_carrier::RetinueCarrier;
    let protected =
        RetinueCarrier::protected(retinue::ifac::Ifac::new(Some("small"), None, 8).unwrap());
    assert_eq!(
        runtime_with_carrier_mtu(2, 10, protected, 200)
            .retinue()
            .node()
            .logical_mtu(),
        200
    );
    assert_eq!(
        runtime_with_carrier_mtu(2, 10, Default::default(), 200)
            .retinue()
            .node()
            .logical_mtu(),
        200
    );
}

#[test]
fn protected_startup_refuses_existing_pending_link_at_identical_mtu() {
    use radio_hand::retinue_carrier::RetinueCarrier;
    use retinue::node::LogicalMtuError;
    let mut node = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[1; 64]),
        DestinationName::new("retinue", ["resident"]).name_hash(),
    );
    node.set_logical_mtu(247).unwrap();
    let peer = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[9; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let blob = retinue::announce::AnnounceBlob::mint([1; 5], 1).unwrap();
    node.ingest(0, &peer.announce(&blob, None), 0);
    node.open_link(peer.destination(), 0, &[8; 64], 0).unwrap();
    let protected =
        RetinueCarrier::protected(retinue::ifac::Ifac::new(Some("pending"), None, 8).unwrap());
    assert_eq!(
        protected.configure_node(&mut node),
        Err(LogicalMtuError::SessionsActive)
    );
    assert_eq!(RetinueCarrier::default().configure_node(&mut node), Ok(()));
}

#[test]
fn carrier_refuses_envelope_flag_as_logical_packet() {
    use radio_hand::retinue_carrier::RetinueCarrier;
    let runtime = runtime(2, 10);
    let blob = retinue::announce::AnnounceBlob::mint([1; 5], 1).unwrap();
    let mut packet = runtime.retinue().node().announce(&blob, None);
    packet.ifac = true;
    // `Packet::encode` never writes the flag, so put it on the frame as a peer would.
    let mut flagged = packet.encode();
    assert_eq!(flagged[0] & 0x80, 0);
    flagged[0] |= 0x80;
    let plain = RetinueCarrier::default();
    assert!(matches!(
        plain.decode(&flagged),
        Err(retinue::Error::BadIfac)
    ));
    assert!(matches!(
        plain.encode(&packet),
        Err(retinue::Error::BadIfac)
    ));
    let protected =
        RetinueCarrier::protected(retinue::ifac::Ifac::new(Some("flag"), None, 8).unwrap());
    assert!(matches!(
        protected.encode(&packet),
        Err(retinue::Error::BadIfac)
    ));
}
