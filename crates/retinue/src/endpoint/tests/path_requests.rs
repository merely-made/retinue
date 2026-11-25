//! Answering and budgeting path requests.

use super::*;
use crate::endpoint::paths::{PATH_REQUEST_GATE, PATH_TABLE_CAPACITY};
use crate::endpoint::{
    IfacePolicy, InterfaceSelector, ListenPolicy, PathRequestPolicy, RoutingPolicy,
};
use crate::packet::HeaderType;

/// A path request for one of our destinations is answered once per tag, on the interface
/// it came in on only. Tagless requests and requests relayed past one hop are ignored.
#[tokio::test]
async fn a_path_request_is_answered_once_on_its_own_interface() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x64; 64]));
    let name = crate::destination::DestinationName::new("retinue", ["pathonce"]);
    let dest = name.destination_hash(ep.identity());
    ep.register(name, b"once");
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    let request = |tag: u8| crate::path::path_request(dest, &[tag; crate::path::TAG_LEN]);
    let take = |iface: &Interface| {
        let packet = iface.outbound.queues.pop();
        if packet.is_some() {
            iface.outbound.queues.delivery_complete();
        }
        packet
    };

    route(&ep.shared, a.id(), request(1));
    let response = take(&a).expect("answered on the requesting interface");
    assert_eq!(response.packet_type, PacketType::Announce);
    assert_eq!(response.context, crate::path::CTX_PATH_RESPONSE);
    assert!(take(&b).is_none(), "and on no other");

    route(&ep.shared, b.id(), request(1));
    let mut tagless = request(2);
    tagless.payload.truncate(crate::hash::ADDRESS_HASH_LEN);
    route(&ep.shared, b.id(), tagless);
    let mut far = request(3);
    far.hops = 2;
    route(&ep.shared, b.id(), far);
    assert!(take(&a).is_none() && take(&b).is_none());
    assert_eq!(ep.routing_counters().filtered_packets, 3);

    // The three-field form a transport-enabled RNS sends still names its tag.
    let mut from_transport = request(4);
    from_transport.payload.splice(16..16, [0xEE; 16]);
    route(&ep.shared, b.id(), from_transport);
    assert!(take(&b).is_some(), "a new tag is answered");
    assert!(take(&a).is_none());
}

#[tokio::test]
async fn answers_a_path_request_for_an_owned_destination() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[9u8; 64]));
    let mut iface = ep.attach_interface();
    let name = crate::destination::DestinationName::new("retinue", ["pathtest"]);
    let dest = name.destination_hash(ep.identity());
    ep.register(name, b"hello");

    // Registration broadcasts a spontaneous announce (context 0); drain it.
    let first = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
        .await
        .expect("registration announce")
        .expect("interface open");
    assert_eq!(first.packet_type, PacketType::Announce);
    assert_eq!(first.context, 0, "a spontaneous announce has context 0");

    // A peer requests a path to our destination.
    let sink = iface.sink();
    assert!(sink.deliver(crate::path::path_request(
        dest,
        &[0x77; crate::path::TAG_LEN]
    )));

    // We answer with a path response: an announce for that destination, context 0x0b.
    let resp = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
        .await
        .expect("path response emitted")
        .expect("interface open");
    assert_eq!(resp.packet_type, PacketType::Announce);
    assert_eq!(resp.context, crate::path::CTX_PATH_RESPONSE);
    assert_eq!(resp.destination, dest);
    // It is a valid announce that reconstructs to our destination and app data.
    let decoded = Announce::decode(&resp).expect("valid announce");
    assert_eq!(decoded.destination, dest);
    assert_eq!(decoded.app_data, b"hello");
    assert_eq!(decoded.identity.hash(), ep.identity().hash());
}

#[tokio::test]
async fn ignores_a_path_request_for_an_unknown_destination() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[10u8; 64]));
    let mut iface = ep.attach_interface();
    let sink = iface.sink();
    let unknown = AddressHash::from_bytes([0xCC; 16]);
    assert!(sink.deliver(crate::path::path_request(
        unknown,
        &[0; crate::path::TAG_LEN]
    )));

    // We own nothing, hold no cache, so we stay silent.
    let got = tokio::time::timeout(Duration::from_millis(200), iface.next_outbound()).await;
    assert!(got.is_err(), "no response for an unknown destination");
}

/// Repeated asking for the same destination broadcasts once, and a different destination
/// is unaffected.
#[tokio::test]
async fn a_path_request_is_rate_limited_per_destination() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[11u8; 64]));
    let mut iface = ep.attach_interface();
    let wanted = AddressHash::from_bytes([0xA1; 16]);
    let other = AddressHash::from_bytes([0xB2; 16]);

    assert!(ep.request_path(wanted), "the first ask goes out");
    assert!(
        !ep.request_path(wanted),
        "an immediate repeat is suppressed"
    );
    assert!(
        ep.request_path(other),
        "a different destination is its own budget"
    );

    let first = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
        .await
        .expect("first request")
        .expect("interface open");
    assert_eq!(first.destination, crate::path::path_request_destination());
    let second = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
        .await
        .expect("the other destination's request")
        .expect("interface open");
    assert_eq!(second.destination, crate::path::path_request_destination());
    let extra = tokio::time::timeout(Duration::from_millis(200), iface.next_outbound()).await;
    assert!(
        extra.is_err(),
        "the suppressed repeat put nothing on the air"
    );

    // Past the floor, asking again is allowed: a destination that never answered must be
    // askable later, or one lost response becomes permanent.
    tokio::time::sleep(PATH_REQUEST_MIN_INTERVAL + Duration::from_millis(20)).await;
    assert!(ep.request_path(wanted), "the floor expires");
}

/// A flood of unique destinations, which the per-destination floor never sees repeat, is
/// held to the global cap, and refused requests do not grow the budget table.
#[tokio::test]
async fn fabricated_unique_destinations_hit_the_global_path_request_cap() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[12u8; 64]));
    let _iface = ep.attach_interface();

    let mut sent = 0;
    for i in 0..(PATH_REQUEST_GLOBAL_MAX as u8 * 4) {
        let mut bytes = [0xD0; 16];
        bytes[0] = i;
        if ep.request_path(AddressHash::from_bytes(bytes)) {
            sent += 1;
        }
    }
    assert_eq!(
        sent, PATH_REQUEST_GLOBAL_MAX,
        "the window admits exactly the cap"
    );
    assert!(
        ep.shared.path_request_budget.lock().unwrap().len() <= PATH_REQUEST_GLOBAL_MAX,
        "refused requests must not grow the budget table",
    );

    // The cap is a window, not a lifetime total: once it slides, asking resumes.
    tokio::time::sleep(PATH_REQUEST_MIN_INTERVAL + Duration::from_millis(20)).await;
    assert!(
        ep.request_path(AddressHash::from_bytes([0xEE; 16])),
        "a fresh window admits new requests",
    );
}

/// An announce for a destination whose path we asked for in the last 45 s is not held
/// behind an ingress burst (`Transport.py` 1815-1821).
#[tokio::test]
async fn a_requested_destination_is_not_held_behind_a_burst() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x66; 64]));
    let wire = ep.attach_interface().id();
    ep.set_announce_ingress_policy(AnnounceIngressPolicy {
        new_interface_hz: 1,
        established_interface_hz: 1,
        ..Default::default()
    });
    for seed in 0x67..0x6A {
        let (packet, _) = peer_announce(seed, "burst");
        route(&ep.shared, wire, packet);
    }
    assert_eq!(ep.announce_ingress_counters(wire).held, 1);

    let (response, requested) = peer_announce(0x6A, "requested");
    assert!(ep.request_path(requested.destination));
    route(&ep.shared, wire, response);
    assert!(ep.route_to(requested.destination).is_some());
    assert_eq!(ep.announce_ingress_counters(wire).held, 1);
    assert!(
        ep.shared
            .path_requested_within(requested.destination, PATH_REQUEST_GATE)
    );
    assert!(
        !ep.shared
            .path_requested_within(requested.destination, Duration::ZERO)
    );
}

async fn next(iface: &mut Interface, within: Duration) -> Option<Packet> {
    tokio::time::timeout(within, iface.next_outbound())
        .await
        .ok()
        .flatten()
}

/// A transport that learns from `source` but relays nothing, so only answers leave.
fn quiet_transport(seed: u8, requesters: Vec<InterfaceId>) -> Endpoint {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[seed; 64]));
    ep.set_routing_policy(RoutingPolicy {
        allowed_ingress: InterfaceSelector::Only(requesters),
        ..RoutingPolicy::transit()
    });
    ep
}

fn relayed(packet: &Packet, via: u8, hops: u8) -> Packet {
    let mut copy = packet.clone();
    copy.header_type = HeaderType::Type2;
    copy.transport = Some(AddressHash::from_bytes([via; 16]));
    copy.hops = hops;
    copy
}

fn request(target: AddressHash, tag: u8) -> Packet {
    crate::path::path_request(target, &[tag; crate::path::TAG_LEN])
}

/// A transport answers from the announce behind a live route, after the grace, on the
/// requesting interface only, as a path response through itself (`Transport.py` 3486-3519).
#[tokio::test(start_paused = true)]
async fn a_transport_answers_from_its_announce_cache() {
    let ep = quiet_transport(0x70, vec![]);
    let mut source = ep.attach_interface();
    let mut asker = ep.attach_interface();
    let (packet, announce) = peer_announce(0x71, "cached");
    route(&ep.shared, source.id(), relayed(&packet, 0xA1, 2));
    let asked = tokio::time::Instant::now();
    route(&ep.shared, asker.id(), request(announce.destination, 1));

    let answer = next(&mut asker, Duration::from_secs(5))
        .await
        .expect("answered");
    assert!(tokio::time::Instant::now() - asked >= Duration::from_millis(400));
    assert_eq!(answer.context, crate::path::CTX_PATH_RESPONSE);
    assert_eq!((answer.header_type, answer.hops), (HeaderType::Type2, 3));
    assert_eq!(answer.transport, Some(ep.identity().hash()));
    assert_eq!(answer.payload, packet.payload);
    assert_eq!(answer.propagation, crate::packet::Propagation::Transport);
    assert!(next(&mut source, Duration::from_secs(5)).await.is_none());
    assert_eq!(ep.routing_counters().path_requests_answered, 1);

    // An ordinary node keeps quiet.
    ep.set_routing_policy(RoutingPolicy::none());
    route(&ep.shared, asker.id(), request(announce.destination, 2));
    assert!(next(&mut asker, Duration::from_secs(5)).await.is_none());
}

/// Requesters on two interfaces inside one grace are both answered: the later answer holds
/// the earlier one back rather than replacing it (`Transport.py` 811-819, 3511-3516).
#[tokio::test(start_paused = true)]
async fn concurrent_requesters_are_each_answered() {
    let ep = quiet_transport(0x7E, vec![]);
    let source = ep.attach_interface();
    let mut first = ep.attach_interface();
    let mut second = ep.attach_interface();
    let (packet, announce) = peer_announce(0x7F, "popular");
    route(&ep.shared, source.id(), relayed(&packet, 0xA1, 2));
    route(&ep.shared, first.id(), request(announce.destination, 1));
    route(&ep.shared, second.id(), request(announce.destination, 2));
    for iface in [&mut second, &mut first] {
        let answer = next(iface, Duration::from_secs(5)).await.expect("answered");
        assert_eq!(answer.context, crate::path::CTX_PATH_RESPONSE);
    }
    assert_eq!(ep.routing_counters().path_requests_answered, 2);
}

/// A route with no cached announce still counts as known: no search goes out (3462-3466).
#[tokio::test(start_paused = true)]
async fn a_known_route_is_never_searched_for() {
    let ep = quiet_transport(0x6E, vec![]);
    let gateway = ep.attach_interface();
    let mut out = ep.attach_interface();
    assert!(ep.set_interface_mode(gateway.id(), InterfaceMode::Gateway));
    let target = AddressHash::from_bytes([0x6F; 16]);
    ep.shared.learn_path(target, out.id(), 1, None);
    route(&ep.shared, gateway.id(), request(target, 1));
    assert!(next(&mut out, Duration::from_secs(1)).await.is_none());
    assert_eq!(ep.routing_counters().path_requests_searched, 0);
}

/// No answer leads back where the request came from: a route learned on the same roaming
/// interface, or one through the requesting transport (`Transport.py` 3468-3484). A roaming
/// requester otherwise waits 1.5 s more.
#[tokio::test(start_paused = true)]
async fn answers_never_point_back_at_the_requester() {
    let ep = quiet_transport(0x72, vec![]);
    let mut wire = ep.attach_interface();
    let mut roaming = ep.attach_interface();
    assert!(ep.set_interface_mode(roaming.id(), InterfaceMode::Roaming));
    let (here, near) = peer_announce(0x73, "near");
    route(&ep.shared, roaming.id(), here);
    route(&ep.shared, roaming.id(), request(near.destination, 1));
    let (there, far) = peer_announce(0x74, "far");
    route(&ep.shared, wire.id(), relayed(&there, 0xB2, 1));
    let mut from_next_hop = request(far.destination, 2);
    from_next_hop.payload.splice(16..16, [0xB2; 16]);
    route(&ep.shared, wire.id(), from_next_hop);
    assert!(next(&mut roaming, Duration::from_secs(5)).await.is_none());
    assert!(next(&mut wire, Duration::from_secs(5)).await.is_none());
    assert_eq!(ep.routing_counters().path_requests_answered, 0);

    let asked = tokio::time::Instant::now();
    route(&ep.shared, roaming.id(), request(far.destination, 3));
    assert!(next(&mut roaming, Duration::from_secs(5)).await.is_some());
    assert!(tokio::time::Instant::now() - asked >= Duration::from_millis(1_900));
}

/// A request for an unknown destination heard on a gateway interface is searched for on the
/// others, with its tag and our transport id; a second requester joins the first, and the
/// announce goes back to both at once (`Transport.py` 1862-1884, 2430-2455, 3528-3583).
#[tokio::test(start_paused = true)]
async fn a_gateway_request_is_searched_and_answered_on_arrival() {
    let ep = quiet_transport(0x75, vec![]);
    let mut gateway = ep.attach_interface();
    let mut out = ep.attach_interface();
    let mut other = ep.attach_interface();
    assert!(ep.set_interface_mode(gateway.id(), InterfaceMode::Gateway));
    let (packet, announce) = peer_announce(0x76, "wanted");
    let target = announce.destination;
    route(&ep.shared, gateway.id(), request(target, 7));
    for iface in [&mut out, &mut other] {
        let search = next(iface, Duration::from_secs(1)).await.expect("searched");
        let parsed = crate::path::PathRequest::parse(&search).unwrap();
        assert_eq!(parsed.requestor, Some(ep.identity().hash()));
        assert_eq!(parsed.tag(), Some(&[7; crate::path::TAG_LEN][..]));
    }
    assert!(next(&mut gateway, Duration::from_secs(1)).await.is_none());
    route(&ep.shared, other.id(), request(target, 8));
    assert!(
        next(&mut out, Duration::from_secs(1)).await.is_none(),
        "one search"
    );
    assert!(ep.shared.discovery.awaits(target));

    route(&ep.shared, out.id(), relayed(&packet, 0xC3, 1));
    for iface in [&mut gateway, &mut other] {
        let answer = next(iface, Duration::from_millis(1))
            .await
            .expect("passed back");
        assert_eq!(
            (answer.context, answer.hops),
            (crate::path::CTX_PATH_RESPONSE, 2)
        );
    }
    assert!(!ep.shared.discovery.awaits(target));
    assert_eq!(ep.routing_counters().path_requests_searched, 1);
}

/// FULL interfaces do not search unless `recursive` is set; BOUNDARY searches BOUNDARY and
/// GATEWAY only (`Transport.py` 3420-3436).
#[tokio::test(start_paused = true)]
async fn the_requesting_mode_decides_the_search() {
    let ep = quiet_transport(0x77, vec![]);
    let asker = ep.attach_interface();
    let mut gateway = ep.attach_interface();
    let mut full = ep.attach_interface();
    assert!(ep.set_interface_mode(gateway.id(), InterfaceMode::Gateway));
    let calm = AnnounceIngressPolicy {
        enabled: false,
        ..AnnounceIngressPolicy::default()
    };
    assert!(ep.set_interface_ingress_policy(asker.id(), Some(calm)));
    let target = AddressHash::from_bytes([0x78; 16]);
    route(&ep.shared, asker.id(), request(target, 1));
    assert!(next(&mut full, Duration::from_secs(1)).await.is_none());

    assert!(ep.set_interface_mode(asker.id(), InterfaceMode::Boundary));
    let target = AddressHash::from_bytes([0x7F; 16]);
    route(&ep.shared, asker.id(), request(target, 2));
    assert!(next(&mut gateway, Duration::from_secs(1)).await.is_some());
    assert!(next(&mut full, Duration::from_secs(1)).await.is_none());

    let recursive = PathRequestPolicy {
        recursive: true,
        ..PathRequestPolicy::default()
    };
    assert!(ep.set_interface_mode(asker.id(), InterfaceMode::Full));
    assert!(ep.set_interface_path_request_policy(asker.id(), recursive));
    route(
        &ep.shared,
        asker.id(),
        request(AddressHash::from_bytes([0x79; 16]), 3),
    );
    assert!(next(&mut full, Duration::from_secs(1)).await.is_some());
    assert_eq!(ep.routing_counters().path_requests_searched, 2);
}

/// Egress control withholds recursive requests past its rate, and a burst of requests on an
/// interface flags it and stops it starting searches (`Interface.py` 213-250).
#[tokio::test]
async fn path_request_rates_limit_both_ways() {
    let ep = quiet_transport(0x7A, vec![]);
    let asker = ep.attach_interface();
    let noisy = ep.attach_interface();
    let out = ep.attach_interface();
    for iface in [asker.id(), noisy.id()] {
        assert!(ep.set_interface_mode(iface, InterfaceMode::Gateway));
    }
    let calm = AnnounceIngressPolicy {
        enabled: false,
        ..AnnounceIngressPolicy::default()
    };
    assert!(ep.set_interface_ingress_policy(asker.id(), Some(calm)));
    let egress = PathRequestPolicy {
        egress_control: true,
        ..PathRequestPolicy::default()
    };
    assert!(ep.set_interface_path_request_policy(out.id(), egress));
    assert!(ep.set_interface_outgoing(noisy.id(), false));
    for seed in 0..6 {
        let target = AddressHash::from_bytes([seed; 16]);
        route(&ep.shared, asker.id(), request(target, seed));
    }
    let counters = ep.routing_counters();
    assert_eq!(counters.path_requests_searched, 6);
    assert_eq!(
        counters.path_requests_egress_limited, 4,
        "two sent, then 3 Hz is passed"
    );
    assert_eq!(out.outbound.queues.depth(), 2);
    assert_eq!(counters.path_requests_ingress_limited, 0);

    for seed in 6..10 {
        let target = AddressHash::from_bytes([seed; 16]);
        route(&ep.shared, noisy.id(), request(target, seed));
    }
    let counters = ep.routing_counters();
    assert_eq!(counters.path_requests_searched, 8, "two before the burst");
    assert_eq!(counters.path_requests_ingress_limited, 2);
    assert_eq!(
        ep.interface_counters(noisy.id())
            .map(|c| c.path_request_burst),
        Some(true)
    );
}

/// Spawned TCP interfaces carry the listener's path-request policy (RNS copies the rates and
/// `recursive_prs`, `TCPInterface.py` 587-650; its `IN` flag is never read in 1.5.7).
#[tokio::test]
async fn spawned_interfaces_inherit_the_path_request_policy() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x7B; 64]));
    let (spawned, mut ids) = tokio::sync::mpsc::channel(1);
    let path_requests = PathRequestPolicy {
        recursive: true,
        egress_control: true,
        egress_hz: 2,
        burst_hz_new: 1,
        burst_hz: 4,
    };
    let iface = IfacePolicy {
        path_requests,
        ..IfacePolicy::default()
    };
    let policy = ListenPolicy {
        mode: InterfaceMode::Gateway,
        iface,
        spawned: Some(spawned),
        ..ListenPolicy::default()
    };
    let listener = ep
        .listen_tcp_with("127.0.0.1:0".parse().unwrap(), policy)
        .await
        .unwrap();
    let _client = tokio::net::TcpStream::connect(listener.local_addr())
        .await
        .unwrap();
    let id = ids.recv().await.unwrap();
    assert_eq!(ep.shared.iface_policy(id).path_requests, path_requests);
}

/// A forgotten route takes its cached announce with it.
#[tokio::test]
async fn the_announce_cache_follows_the_route() {
    let ep = quiet_transport(0x7C, vec![]);
    let iface = ep.attach_interface();
    let (packet, announce) = peer_announce(0x7D, "evicted");
    route(&ep.shared, iface.id(), packet);
    assert!(
        ep.shared
            .discovery
            .announces
            .lock()
            .unwrap()
            .contains_key(&announce.destination)
    );
    ep.shared.forget_path(announce.destination);
    assert!(ep.shared.discovery.announces.lock().unwrap().is_empty());

    // Evicted from a full table, a route drops its announce at once.
    for seed in 0x60..0x60 + PATH_TABLE_CAPACITY as u8 + 1 {
        route(&ep.shared, iface.id(), peer_announce(seed, "filler").0);
    }
    let routed: std::collections::HashSet<_> = ep
        .shared
        .path_table
        .lock()
        .unwrap()
        .keys()
        .copied()
        .collect();
    let cache = ep.shared.discovery.announces.lock().unwrap();
    assert_eq!(
        cache
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>(),
        routed
    );
}
