//! Relayed announces as both transport drivers handle them.

use super::*;

#[tokio::test(start_paused = true)]
async fn interface_removal_purges_return_paths_and_pending_relays() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();
    ep.set_relay_jitter(Duration::ZERO);
    let (lost, _) = peer_announce(0x7d, "removed");
    let (kept, _) = peer_announce(0x7e, "retained");
    for packet in [&lost, &kept] {
        ep.shared.learn_path(packet.destination, b.id(), 0, None);
    }
    // The pending relay was heard on a, but its destination is still reachable on b.
    ep.shared.schedule_rebroadcast(a.id(), lost.clone(), 0);
    ep.shared.schedule_rebroadcast(b.id(), kept.clone(), 0);
    assert!(
        ep.shared
            .schedule_path_response(a.id(), kept.clone(), Duration::ZERO)
    );
    for (byte, received, outbound) in [
        (1, a.id(), b.id()),
        (2, b.id(), a.id()),
        (3, b.id(), b.id()),
    ] {
        ep.shared
            .remember_reverse(AddressHash::from_bytes([byte; 16]), received, outbound);
    }
    let survivor = AddressHash::from_bytes([3; 16]);
    let expected = ep.shared.reverse_table.lock().unwrap()[&survivor];
    ep.detach_interface(a.id());
    assert_eq!(
        *ep.shared.reverse_table.lock().unwrap(),
        [(survivor, expected)].into()
    );
    assert_eq!(
        ep.shared.live_route(lost.destination).unwrap().iface,
        b.id()
    );
    assert!(
        !ep.shared
            .rebroadcasts
            .lock()
            .unwrap()
            .scheduled(lost.destination)
    );
    assert!(
        ep.shared
            .rebroadcasts
            .lock()
            .unwrap()
            .scheduled(kept.destination)
    );
    ep.shared.release_rebroadcasts(1);
    let restored = b
        .outbound
        .queues
        .pop()
        .expect("unrelated held relay survives");
    assert_eq!(restored.destination, kept.destination);
    assert!(b.outbound.queues.pop().is_none());
    ep.detach_interface(a.id());
    assert_eq!(ep.shared.take_reverse(survivor, b.id()), Some(b.id()));
}

#[tokio::test(start_paused = true)]
async fn interface_removal_purges_cap_queues_by_recorded_interface() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();
    ep.set_relay_jitter(Duration::ZERO);
    for iface in [a.id(), b.id()] {
        ep.set_first_hop_airtime(iface, Duration::from_millis(100));
        assert!(ep.shared.admit_recursive_path_request(iface, 500));
    }
    let (lost, _) = peer_announce(0x7d, "removed");
    let (kept, _) = peer_announce(0x7e, "retained");
    for (packet, from) in [(&lost, a.id()), (&kept, b.id())] {
        ep.shared.learn_path(packet.destination, b.id(), 0, None);
        ep.shared.schedule_rebroadcast(from, packet.clone(), 0);
    }
    ep.shared.release_rebroadcasts(0);
    assert_eq!(ep.routing_counters().capped_announces, 4);
    assert!(b.outbound.queues.pop().is_none());
    ep.detach_interface(a.id());
    ep.shared.release_rebroadcasts(10_000);
    let sent = b
        .outbound
        .queues
        .pop()
        .expect("unrelated queued relay survives");
    assert_eq!(sent.destination, kept.destination);
    assert!(
        b.outbound.queues.pop().is_none(),
        "removed ingress leaves no queued copy"
    );
}

#[tokio::test(start_paused = true)]
async fn a_repeated_announce_over_the_hop_ceiling_is_remembered_before_it_is_counted() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();
    let (mut announce, decoded) = peer_announce(0x7f, "ceiling");
    announce.hops = MAX_HOPS - 1;
    route(&ep.shared, a.id(), announce.clone());
    assert_eq!(ep.routing_counters().hop_limit_dropped, 1);
    // Without a live route the copy is a first sighting and reaches the relay gate again.
    ep.shared.forget_path(decoded.destination);
    route(&ep.shared, a.id(), announce);
    assert!(ep.route_to(decoded.destination).is_some(), "learned again");
    assert_eq!(
        ep.routing_counters().hop_limit_dropped,
        1,
        "the repeat is filtered, not counted again"
    );
    assert!(a.outbound.queues.pop().is_none());
    assert!(b.outbound.queues.pop().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_relay_leaves_on_every_permitted_interface_the_arrival_one_included() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    let c = ep.attach_interface();
    ep.enable_routing();
    ep.set_relay_jitter(Duration::ZERO);
    assert!(ep.set_interface_mode(c.id(), InterfaceMode::AccessPoint));
    let (announce, decoded) = peer_announce(0x70, "fanout");
    route(&ep.shared, a.id(), announce);
    ep.shared
        .release_rebroadcasts(ep.shared.announce_admission_now_ms());
    for iface in [&a, &b] {
        let out = iface.outbound.queues.pop().expect("relayed here");
        assert_eq!(out.destination, decoded.destination);
    }
    assert!(c.outbound.queues.pop().is_none(), "not the access point");
    // The desktop counts a transmission once (inventory row j, D23).
    assert_eq!(ep.routing_counters().forwarded_announces, 1);
}
