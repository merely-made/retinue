//! Transit scheduling: a busy neighbour cannot spend the capacity a host reserved for itself.
//!
//! V8 (scheduling half) of the 2026-07-24 low-power radio and managed-network plan. The policy
//! half decides *whether* traffic is carried; this decides what happens when more is offered
//! than the interface can carry at once, which on a radio is most of the time. The queue is
//! drained by hand here rather than by a socket, so the outcome is the schedule's and not a
//! race.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, Interface, QueueDepths, QueueWeights, RoutingPolicy};
use retinue::identity::PrivateIdentity;
use retinue::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};

fn hub() -> (Endpoint, Interface, Interface) {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[9u8; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    (ep, a, b)
}

fn transit_packet(hub: &Endpoint, dest: retinue::hash::AddressHash) -> Packet {
    Packet {
        ifac: false,
        header_type: HeaderType::Type2,
        context_flag: false,
        propagation: Propagation::Transport,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport: Some(hub.identity().hash()),
        destination: dest,
        context: 0,
        payload: vec![0xAB; 200],
    }
}

/// Teach `hub` a route to a peer's destination reachable on interface `via`.
async fn teach_route(hub: &Endpoint, via: &Interface, peer_seed: u8) -> retinue::hash::AddressHash {
    let peer = PrivateIdentity::from_secret_bytes(&[peer_seed; 64]);
    let name = DestinationName::new("leaf", ["svc"]);
    let dest = name.destination_hash(peer.public());

    let peer_ep = Endpoint::new(peer.clone());
    let mut peer_iface = peer_ep.attach_interface();
    peer_ep.register(name, b"svc");
    let announce = tokio::time::timeout(Duration::from_secs(2), peer_iface.next_outbound())
        .await
        .expect("peer announce")
        .expect("interface open");
    assert!(via.sink().deliver(announce));
    for _ in 0..20 {
        if hub.route_to(dest).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(hub.route_to(dest).is_some(), "route should be learned");
    dest
}

/// Offer far more transit than the interface can carry, while the host announces its own
/// destinations, then drain: the host's own control traffic still gets out, and it does so
/// without waiting for the transit backlog to clear.
///
/// This is V8's headline: transit is served, but never ahead of the node's own traffic.
#[tokio::test]
async fn sustained_transit_cannot_starve_local_traffic() {
    let (hub, a, mut b) = hub();
    let dest = teach_route(&hub, &b, 2).await;
    hub.enable_routing();

    // A neighbour floods transit toward `dest`, all of which must leave on interface b.
    for _ in 0..40 {
        assert!(a.sink().deliver(transit_packet(&hub, dest)));
    }
    // Give the router time to queue it all.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Meanwhile the host announces its own destination — its own control traffic.
    let own = DestinationName::new("hub", ["svc"]);
    hub.register(own.clone(), b"svc");
    let own_dest = own.destination_hash(hub.identity());
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Drain a short prefix of the interface, as a slow radio would.
    let mut saw_own_control_at = None;
    let mut drained = 0;
    for i in 0..12 {
        let pkt = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
            .await
            .expect("the interface should have queued traffic")
            .expect("interface open");
        drained += 1;
        if pkt.packet_type == PacketType::Announce && pkt.destination == own_dest {
            saw_own_control_at = Some(i);
            break;
        }
    }

    let at = saw_own_control_at.expect("the host's own announce must escape the transit flood");
    assert!(
        at < 8,
        "own control traffic waited behind {at} transit packets; \
         it must not queue behind a 40-packet backlog"
    );
    assert!(drained > 0);
}

/// Weights are shares of a contended interface: with transit and local traffic both backed up,
/// the drained order reflects the configured ratio rather than arrival order.
#[tokio::test]
async fn drained_order_follows_the_configured_shares() {
    let (hub, a, mut b) = hub();
    let dest = teach_route(&hub, &b, 2).await;

    // Give control a large share and transit a small one, so the ratio is unmistakable.
    hub.set_routing_policy(RoutingPolicy {
        queue_weights: QueueWeights {
            control: 8,
            interactive: 4,
            background: 2,
            transit: 1,
        },
        ..RoutingPolicy::transit()
    });

    // Back both classes up deeply, so the drain measures the *ratio* rather than simply
    // exhausting a short queue. Transit is offered first, so arrival order alone would put
    // all of it ahead.
    for _ in 0..30 {
        assert!(a.sink().deliver(transit_packet(&hub, dest)));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let own = DestinationName::new("hub", ["svc"]);
    hub.register(own.clone(), b"svc");
    for _ in 0..30 {
        hub.announce(&own, b"svc");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Drain a stretch with both classes still backed up throughout, and count the split.
    let mut control = 0;
    let mut transit = 0;
    for _ in 0..22 {
        let pkt = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
            .await
            .expect("queued traffic")
            .expect("interface open");
        if pkt.packet_type == PacketType::Announce && pkt.hops == 0 {
            control += 1;
        } else {
            transit += 1;
        }
    }
    assert!(
        control >= transit * 3,
        "with both classes backed up at 8:1, control ({control}) should take a clear \
         majority over transit ({transit}), despite transit arriving first"
    );
    assert!(
        transit > 0,
        "transit is bounded, not blocked: it must still get a share"
    );
    let counters = hub.queue_counters();
    assert!(counters.sent.control > 0, "control was released");
    assert!(
        counters.sent.transit > 0,
        "transit was still served, not starved out"
    );
}

/// Queues are bounded, and the bound is reported. A neighbour offering more transit than the
/// configured depth has the excess dropped rather than accumulated.
#[tokio::test]
async fn transit_queue_depth_is_bounded_and_drops_are_counted() {
    let (hub, a, mut b) = hub();
    let dest = teach_route(&hub, &b, 2).await;

    hub.set_routing_policy(RoutingPolicy {
        queue_depths: QueueDepths {
            transit: 8,
            ..QueueDepths::DEFAULT
        },
        ..RoutingPolicy::transit()
    });

    // Offer well past the depth without draining.
    for _ in 0..50 {
        assert!(a.sink().deliver(transit_packet(&hub, dest)));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let counters = hub.queue_counters();
    assert!(
        counters.dropped.transit > 0,
        "a bounded transit queue must drop the excess, not grow"
    );
    assert_eq!(
        counters.dropped.control, 0,
        "the host's own control traffic must not be dropped by a neighbour's flood"
    );

    // What was kept is still deliverable, and bounded by the configured depth.
    let mut delivered = 0;
    while tokio::time::timeout(Duration::from_millis(150), b.next_outbound())
        .await
        .is_ok()
    {
        delivered += 1;
        if delivered > 20 {
            break;
        }
    }
    assert!(
        delivered <= 12,
        "delivered {delivered}, which exceeds the configured transit depth"
    );
}

/// Turning transit off entirely leaves the host's own traffic flowing at full rate: the
/// scheduler is not a tax on a node that carries nothing.
#[tokio::test]
async fn disabling_transit_leaves_local_traffic_untouched() {
    let (hub, _a, mut b) = hub();
    assert_eq!(hub.routing_policy(), RoutingPolicy::none());

    let own = DestinationName::new("hub", ["svc"]);
    hub.register(own.clone(), b"svc");
    for _ in 0..5 {
        hub.announce(&own, b"svc");
    }

    let mut seen = 0;
    while tokio::time::timeout(Duration::from_millis(200), b.next_outbound())
        .await
        .is_ok()
    {
        seen += 1;
        if seen >= 6 {
            break;
        }
    }
    assert!(
        seen >= 5,
        "a non-carrying node should emit all its own announces, saw {seen}"
    );
    assert_eq!(
        hub.queue_counters().dropped.control,
        0,
        "no local drops on an idle interface"
    );
}
