use super::*;

#[test]
fn preserve_sessions_refuses_without_calling_nonce_source() {
    let (mut a, _b, id) = linked();
    let before = a.pause_assessment();
    let mut called = false;
    let result = a.force_interrupt(InterruptionPermission::PreserveSessions, || {
        called = true;
        [0xF1; 16]
    });
    assert!(result.is_err());
    assert!(!called);
    assert_eq!(a.pause_assessment(), before);
    assert!(a.has_link(id));
}

#[test]
fn forced_interruption_reports_and_clears_pending_link() {
    let (mut a, mut b) = pair();
    a.ingest(
        IFACE,
        &b.announce(
            &retinue::announce::AnnounceBlob::from_wire([7; RAND_HASH_LEN]),
            None,
        ),
        0,
    );
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x61; 64], 0).unwrap());
    let mut calls = 0;
    let report = a
        .force_interrupt(InterruptionPermission::AllowSessionLoss, || {
            calls += 1;
            [0x62; 16]
        })
        .unwrap();
    assert_eq!(report.pending_links.len(), 1);
    assert_eq!(calls, 0);
    let proof = sent(&b.ingest(IFACE, &request, 1));
    assert!(a.ingest(IFACE, &proof, 2).is_empty());
    assert_eq!(a.link_count(), 0);
}

#[test]
fn forced_interruption_closes_link_and_allows_fresh_handshake() {
    let (mut a, mut b, id) = linked();
    let report = a
        .force_interrupt(InterruptionPermission::AllowSessionLoss, || [0x71; 16])
        .unwrap();
    assert_eq!(report.closed_links.as_slice(), &[id]);
    let close = &report.close_packets[0];
    assert!(
        b.ingest(IFACE, close, 1)
            .iter()
            .any(|action| matches!(action, Action::LinkDown { link_id } if *link_id == id))
    );
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x72; 64], 1).unwrap());
    let proof = sent(&b.ingest(IFACE, &request, 2));
    let new_id = link_up(&a.ingest(IFACE, &proof, 3));
    assert_ne!(new_id, id, "fresh handshake must use a new link identity");
    assert!(a.has_link(new_id));
}

#[test]
fn forced_interruption_drops_resource_state_and_late_packet_is_ignored() {
    let (mut a, mut b, id) = linked();
    let started = a
        .publish(id, IFACE, &[0xB3; 1_024], [0xB4; 4], &[0xB5; 16], 0)
        .unwrap();
    let advertisement = sent(&started);
    let held_advertisement = advertisement.clone();
    let offer = b.ingest(IFACE, &advertisement, 1);
    let request = sent(&offer);
    assert_eq!(b.pause_assessment().inbound_resources, 1);
    let report = a
        .force_interrupt(InterruptionPermission::AllowSessionLoss, || [0xB6; 16])
        .unwrap();
    assert_eq!(report.outbound_resources.as_slice(), &[id]);
    let late = a.ingest(IFACE, &request, 2);
    assert!(late.is_empty());
    assert_eq!(sent(&started), held_advertisement);
    assert_eq!(a.link_count(), 0);
    assert!(b.transfer_active(id));
}

#[test]
fn forced_report_uses_its_own_capacity_for_multiple_links() {
    type Tiny = Node<32, 1, 4>;
    let mut center = Tiny::new(
        PrivateIdentity::from_secret_bytes(&[0xC1; 64]),
        DestinationName::new("retinue", ["tiny-center"]).name_hash(),
    );
    let mut peers: [Node<32, 8, 4>; 2] = [
        Node::new(
            PrivateIdentity::from_secret_bytes(&[0xC2; 64]),
            DestinationName::new("retinue", ["tiny-peer-a"]).name_hash(),
        ),
        Node::new(
            PrivateIdentity::from_secret_bytes(&[0xC3; 64]),
            DestinationName::new("retinue", ["tiny-peer-b"]).name_hash(),
        ),
    ];
    for (index, peer) in peers.iter_mut().enumerate() {
        center.ingest(
            IFACE,
            &peer.announce(
                &retinue::announce::AnnounceBlob::from_wire([index as u8 + 20; RAND_HASH_LEN]),
                None,
            ),
            index as u64,
        );
        let request = sent(
            &center
                .open_link(
                    peer.destination(),
                    IFACE,
                    &[index as u8 + 40; 64],
                    index as u64,
                )
                .unwrap(),
        );
        let proof = sent(&peer.ingest(IFACE, &request, index as u64));
        assert!(
            center
                .ingest(IFACE, &proof, index as u64)
                .iter()
                .any(|action| matches!(action, Action::LinkUp { .. }))
        );
    }
    assert_eq!(center.link_count(), 2);
    let report = center
        .force_interrupt(InterruptionPermission::AllowSessionLoss, || [0xD1; 16])
        .unwrap();
    assert_eq!(report.closed_links.len(), 2);
    assert_eq!(report.close_packets.len(), 2);
    assert_eq!(center.link_count(), 0);
}
