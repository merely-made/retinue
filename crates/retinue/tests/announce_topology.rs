//! The same signed announce in the two host models. A Node shell drives one shared radio;
//! an Endpoint relays on every permitted interface, the ingress one included, as RNS does.
#![cfg(feature = "tokio")]

use std::time::Duration;

use retinue::announce::AnnounceBlob;
use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, Interface, InterfaceSelector, RoutingPolicy};
use retinue::identity::PrivateIdentity;
use retinue::node::{Action, Node, TransportConfig};
use retinue::packet::{HeaderType, Packet, PacketType};

fn signed_announce_from(seed: u8, aspect: &'static str, timebase: u8) -> Packet {
    let peer = Node::<8, 8, 4, 8>::new(
        PrivateIdentity::from_secret_bytes(&[seed; 64]),
        DestinationName::new("topology", [aspect]).name_hash(),
    );
    peer.announce(
        &AnnounceBlob::from_wire([timebase, 0, 0, 0, 0, 0, 0, 0, 0, timebase]),
        None,
    )
}

fn signed_announce(timebase: u8) -> Packet {
    signed_announce_from(0x52, "peer", timebase)
}

async fn wait_for_route(ep: &Endpoint, destination: retinue::hash::AddressHash) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if ep.route_to(destination).is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("verified announce should teach a route");
}

async fn outbound(iface: &mut Interface) -> Packet {
    tokio::time::timeout(Duration::from_secs(2), iface.next_outbound())
        .await
        .expect("relay should be emitted")
        .expect("interface should remain open")
}

#[tokio::test]
async fn equivalent_announce_rebroadcasts_on_the_radio_or_every_interface() {
    let first = signed_announce(10);
    let destination = first.destination;

    let node_identity = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let mut node = Node::<8, 8, 4, 8>::new(
        node_identity.clone(),
        DestinationName::new("topology", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    assert!(
        node.ingest(17, &first, 0)
            .iter()
            .all(|action| !matches!(action, Action::Send { .. }))
    );
    let actions = node.poll(node.next_rebroadcast().expect("scheduled"), 17, None);
    let sends: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            Action::Send { interface, packet } => Some((*interface, packet)),
            _ => None,
        })
        .collect();
    assert_eq!(sends.len(), 1, "one shared radio rebroadcast");
    assert_eq!(sends[0].0, 17, "the shell transmits on the ingress radio");
    assert_eq!(sends[0].1.destination, destination);
    assert_eq!(sends[0].1.hops, 1);
    assert_eq!(sends[0].1.header_type, HeaderType::Type2);
    assert_eq!(sends[0].1.transport, Some(node_identity.hash()));
    assert_eq!(node.route_to(destination, 0), Some((17, 0)));

    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x62; 64]));
    let mut ingress = ep.attach_interface();
    let mut other_a = ep.attach_interface();
    let mut other_b = ep.attach_interface();
    ep.set_routing_policy(RoutingPolicy::transit());
    assert!(ingress.sink().deliver(first.clone()));
    wait_for_route(&ep, destination).await;
    assert_eq!(ep.route_to(destination), Some((ingress.id(), 0)));
    for relayed in [
        outbound(&mut ingress).await,
        outbound(&mut other_a).await,
        outbound(&mut other_b).await,
    ] {
        assert_eq!(relayed.packet_type, PacketType::Announce);
        assert_eq!(relayed.destination, destination);
        assert_eq!(relayed.hops, 1);
        assert_eq!(relayed.header_type, HeaderType::Type2);
        assert_eq!(relayed.transport, Some(ep.identity().hash()));
    }

    // Replaying the identical signed packet from another interface changes neither route nor
    // recipients. Suppression precedes learning and relay in both runtimes.
    assert!(node.ingest(18, &first, 1).is_empty());
    assert_eq!(node.route_to(destination, 1), Some((17, 0)));
    assert!(other_a.sink().deliver(first));
    let sentinel = signed_announce_from(0x53, "sentinel", 11);
    let sentinel_destination = sentinel.destination;
    assert!(other_a.sink().deliver(sentinel));
    wait_for_route(&ep, sentinel_destination).await;
    assert_eq!(ep.route_to(destination), Some((ingress.id(), 0)));
    assert_eq!(
        outbound(&mut ingress).await.destination,
        sentinel_destination,
        "nothing from the duplicate precedes the sentinel"
    );
    assert_eq!(
        outbound(&mut other_b).await.destination,
        sentinel_destination,
        "nothing from the duplicate precedes the sentinel"
    );
    assert_eq!(node.transport_counters().forwarded_announces, 1);
    assert_eq!(ep.routing_counters().forwarded_announces, 2);
}

#[tokio::test]
async fn endpoint_egress_policy_selects_recipients_without_changing_learning() {
    let announce = signed_announce(20);
    let destination = announce.destination;
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x63; 64]));
    let ingress = ep.attach_interface();
    let mut allowed = ep.attach_interface();
    let mut refused = ep.attach_interface();
    ep.set_routing_policy(RoutingPolicy {
        allowed_egress: InterfaceSelector::Only(vec![allowed.id()]),
        ..RoutingPolicy::transit()
    });

    assert!(ingress.sink().deliver(announce));
    wait_for_route(&ep, destination).await;
    assert_eq!(ep.route_to(destination), Some((ingress.id(), 0)));
    assert_eq!(outbound(&mut allowed).await.destination, destination);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), refused.next_outbound())
            .await
            .is_err(),
        "a disallowed interface receives no relay"
    );
    assert_eq!(ep.routing_counters().forwarded_announces, 1);
}
