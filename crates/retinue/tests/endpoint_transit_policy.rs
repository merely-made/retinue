//! Transit policy: what an endpoint carries for others is an owner setting, not a switch.
//!
//! V8 (policy half) of the 2026-07-24 low-power radio and managed-network plan. A node that
//! sells "carry transit without exposing owner data" has to be able to say *which* traffic,
//! *from* where, *to* where, and *how far* — independently. These tests drive the router
//! through the raw interface seam (no sockets, no timing) and assert the policy matrix plus
//! the counters that prove it was enforced.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, Interface, InterfaceSelector, RoutingPolicy};
use retinue::identity::PrivateIdentity;
use retinue::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};

/// A hub with two raw interfaces: `(endpoint, a, b)`.
fn hub() -> (Endpoint, Interface, Interface) {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[9u8; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    (ep, a, b)
}

/// A transit-shaped packet: header-type-2, addressed to `hub` as the hop, bound for `dest`.
fn transit_packet(hub: &Endpoint, dest: retinue::hash::AddressHash, hops: u8) -> Packet {
    Packet {
        ifac: false,
        header_type: HeaderType::Type2,
        context_flag: false,
        propagation: Propagation::Transport,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops,
        transport: Some(hub.identity().hash()),
        destination: dest,
        context: 0,
        payload: b"someone else's traffic".to_vec(),
    }
}

/// Teach the hub a route to `dest` on interface `via` by feeding it a signed announce from a
/// peer that owns that destination, then return the destination hash.
async fn teach_route(
    hub: &Endpoint,
    via: &Interface,
    peer_seed: u8,
    aspect: &'static str,
) -> retinue::hash::AddressHash {
    let peer = PrivateIdentity::from_secret_bytes(&[peer_seed; 64]);
    let name = DestinationName::new("leaf", [aspect]);
    let dest = name.destination_hash(peer.public());

    // Build the announce with a peer-side endpoint, then inject it as if it arrived on `via`.
    let peer_ep = Endpoint::new(peer.clone());
    let mut peer_iface = peer_ep.attach_interface();
    peer_ep.register(name, aspect.as_bytes());
    let announce = tokio::time::timeout(Duration::from_secs(2), peer_iface.next_outbound())
        .await
        .expect("peer should emit its registration announce")
        .expect("interface open");

    assert!(
        via.sink().deliver(announce),
        "hub should accept the announce"
    );
    // Let the router ingest it.
    for _ in 0..20 {
        if hub.route_to(dest).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        hub.route_to(dest).is_some(),
        "hub should have learned a route to {dest}"
    );
    dest
}

/// Nothing is carried by default: a fresh endpoint refuses transit and says so.
#[tokio::test]
async fn default_policy_carries_nothing() {
    let (hub, a, mut b) = hub();
    assert_eq!(hub.routing_policy(), RoutingPolicy::none());

    let dest = teach_route(&hub, &b, 2, "b").await;
    assert!(a.sink().deliver(transit_packet(&hub, dest, 0)));

    // Nothing leaves the far interface, and the refusal is counted.
    let out = tokio::time::timeout(Duration::from_millis(300), b.next_outbound()).await;
    assert!(out.is_err(), "a default endpoint must not carry transit");
    assert_eq!(hub.routing_counters().policy_rejected, 1);
    assert_eq!(hub.routing_counters().forwarded_packets, 0);
}

/// `enable_routing` remains exactly the full-transit shorthand it always was.
#[tokio::test]
async fn enable_routing_is_full_transit() {
    let (hub, a, mut b) = hub();
    hub.enable_routing();
    assert_eq!(hub.routing_policy(), RoutingPolicy::transit());

    let dest = teach_route(&hub, &b, 2, "b").await;
    assert!(a.sink().deliver(transit_packet(&hub, dest, 0)));

    let out = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
        .await
        .expect("transit should be carried")
        .expect("interface open");
    assert_eq!(out.destination, dest);
    assert_eq!(out.hops, 1, "a forwarded packet counts the hop");
    assert_eq!(hub.routing_counters().forwarded_packets, 1);
    assert_eq!(hub.routing_counters().policy_rejected, 0);
}

/// Ingress is directional: transit from a permitted interface is carried, and the same
/// traffic arriving on a refused one is not.
#[tokio::test]
async fn ingress_selection_is_enforced_per_interface() {
    let (hub, a, mut b) = hub();
    let dest = teach_route(&hub, &b, 2, "b").await;

    // Accept transit only from interface `a`.
    hub.set_routing_policy(RoutingPolicy {
        allowed_ingress: InterfaceSelector::Only(vec![a.id()]),
        ..RoutingPolicy::transit()
    });

    assert!(a.sink().deliver(transit_packet(&hub, dest, 0)));
    let carried = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
        .await
        .expect("transit from a permitted ingress should be carried")
        .expect("interface open");
    assert_eq!(carried.destination, dest);
    assert_eq!(hub.routing_counters().forwarded_packets, 1);

    // The same packet arriving on `b` (not permitted) is refused, not carried back out.
    assert!(b.sink().deliver(transit_packet(&hub, dest, 0)));
    let refused = tokio::time::timeout(Duration::from_millis(300), b.next_outbound()).await;
    assert!(refused.is_err(), "transit from a refused ingress must drop");
    assert_eq!(hub.routing_counters().policy_rejected, 1);
    assert_eq!(
        hub.routing_counters().forwarded_packets,
        1,
        "the refused packet must not be counted as forwarded"
    );
}

/// Egress is independently selectable: a node may accept transit from anywhere and still
/// refuse to emit it on a particular interface.
#[tokio::test]
async fn egress_selection_is_enforced_per_interface() {
    let (hub, a, mut b) = hub();
    let dest = teach_route(&hub, &b, 2, "b").await;

    // Accept from anywhere, but never emit on `b` — the only route to `dest`.
    hub.set_routing_policy(RoutingPolicy {
        allowed_egress: InterfaceSelector::Only(vec![a.id()]),
        ..RoutingPolicy::transit()
    });

    assert!(a.sink().deliver(transit_packet(&hub, dest, 0)));
    let out = tokio::time::timeout(Duration::from_millis(300), b.next_outbound()).await;
    assert!(out.is_err(), "a refused egress must not emit");
    assert_eq!(hub.routing_counters().policy_rejected, 1);
    assert_eq!(hub.routing_counters().forwarded_packets, 0);
}

/// The hop ceiling bounds how far this node will carry anything, independently of whether it
/// carries at all.
#[tokio::test]
async fn max_hops_bounds_what_is_carried() {
    let (hub, a, mut b) = hub();
    let dest = teach_route(&hub, &b, 2, "b").await;

    hub.set_routing_policy(RoutingPolicy {
        max_hops: 3,
        ..RoutingPolicy::transit()
    });

    // Under the ceiling: carried.
    assert!(a.sink().deliver(transit_packet(&hub, dest, 2)));
    let carried = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
        .await
        .expect("a packet under the ceiling should be carried")
        .expect("interface open");
    assert_eq!(carried.hops, 3);

    // At the ceiling: dropped and counted distinctly from a policy refusal.
    assert!(a.sink().deliver(transit_packet(&hub, dest, 3)));
    let dropped = tokio::time::timeout(Duration::from_millis(300), b.next_outbound()).await;
    assert!(
        dropped.is_err(),
        "a packet at the ceiling must not be carried"
    );
    let counters = hub.routing_counters();
    assert_eq!(counters.hop_limit_dropped, 1);
    assert_eq!(
        counters.policy_rejected, 0,
        "a hop drop is not a policy refusal"
    );
    assert_eq!(counters.forwarded_packets, 1);
}

/// Announce relaying and packet forwarding are separate axes: a node can keep its neighbours
/// discoverable while refusing to carry their data.
#[tokio::test]
async fn announces_relay_while_packets_are_refused() {
    let (hub, a, mut b) = hub();
    let dest = teach_route(&hub, &b, 2, "b").await;

    hub.set_routing_policy(RoutingPolicy {
        forward_packets: false,
        ..RoutingPolicy::transit()
    });

    // Data transit is refused...
    assert!(a.sink().deliver(transit_packet(&hub, dest, 0)));
    let refused = tokio::time::timeout(Duration::from_millis(300), b.next_outbound()).await;
    assert!(refused.is_err(), "packet transit is disabled");
    assert_eq!(hub.routing_counters().policy_rejected, 1);

    // ...but an announce arriving on `a` is still relayed out `b`, stamped through us.
    let far = teach_route(&hub, &a, 3, "c").await;
    let relayed = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
        .await
        .expect("announce relaying is still enabled")
        .expect("interface open");
    assert_eq!(relayed.packet_type, PacketType::Announce);
    assert_eq!(relayed.destination, far);
    assert_eq!(relayed.hops, 1, "a relayed announce counts the hop");
    assert_eq!(
        relayed.transport,
        Some(hub.identity().hash()),
        "a relayed announce is stamped with the relaying node"
    );
    assert_eq!(hub.routing_counters().forwarded_announces, 1);
    assert_eq!(
        hub.routing_counters().forwarded_packets,
        0,
        "relaying an announce is not forwarding a packet"
    );
}

/// Relay jitter delays an announce relay without losing it. Every neighbour that hears an
/// announce relays it, so relaying instantly means relaying simultaneously; spreading them in
/// time is the cheapest defence against a flood colliding with itself.
#[tokio::test]
async fn relay_jitter_delays_the_relay_without_dropping_it() {
    let (hub, a, mut b) = hub();
    hub.enable_routing();
    hub.set_relay_jitter(Duration::from_millis(300));

    let started = tokio::time::Instant::now();
    let far = teach_route(&hub, &a, 3, "c").await;

    let relayed = tokio::time::timeout(Duration::from_secs(3), b.next_outbound())
        .await
        .expect("a jittered relay still arrives")
        .expect("interface open");
    assert_eq!(relayed.packet_type, PacketType::Announce);
    assert_eq!(relayed.destination, far);
    assert_eq!(relayed.hops, 1);
    assert_eq!(
        hub.routing_counters().forwarded_announces,
        1,
        "a jittered relay is still counted, once"
    );
    // It cannot have arrived before the announce was even injected.
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// Jitter off is the default, so a point-to-point link pays no latency for a defence it does
/// not need.
#[tokio::test]
async fn relay_jitter_is_off_by_default() {
    let (hub, a, mut b) = hub();
    hub.enable_routing();

    let far = teach_route(&hub, &a, 3, "c").await;
    // With no jitter the relay is already queued by the time the route is learned.
    let relayed = tokio::time::timeout(Duration::from_millis(500), b.next_outbound())
        .await
        .expect("an unjittered relay goes out immediately")
        .expect("interface open");
    assert_eq!(relayed.destination, far);
}

/// Turning transit off leaves the endpoint's own service untouched: it still announces its
/// own destinations and answers its own path requests.
#[tokio::test]
async fn disabling_transit_does_not_affect_local_service() {
    let (hub, _a, mut b) = hub();
    assert_eq!(hub.routing_policy(), RoutingPolicy::none());

    let name = DestinationName::new("hub", ["svc"]);
    let dest = name.destination_hash(hub.identity());
    hub.register(name, b"svc");

    let own = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
        .await
        .expect("a non-routing endpoint still announces its own destinations")
        .expect("interface open");
    assert_eq!(own.packet_type, PacketType::Announce);
    assert_eq!(own.destination, dest);
    assert_eq!(own.hops, 0, "our own announce originates here");

    // And it answers a path request for its own destination while carrying nothing.
    assert!(
        b.sink()
            .deliver(retinue::path::path_request(dest, &[0x5A; 16]))
    );
    let response = tokio::time::timeout(Duration::from_secs(2), b.next_outbound())
        .await
        .expect("a path response is local service, not transit")
        .expect("interface open");
    assert_eq!(response.packet_type, PacketType::Announce);
    assert_eq!(response.context, retinue::path::CTX_PATH_RESPONSE);
    assert_eq!(
        hub.routing_counters().forwarded_packets,
        0,
        "answering for ourselves is not transit"
    );
}
