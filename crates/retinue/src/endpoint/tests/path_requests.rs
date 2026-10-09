//! Answering and budgeting path requests.

use super::*;
use crate::endpoint::paths::PATH_REQUEST_GATE;

/// A path request for one of our destinations is answered once per tag, on the interface
/// it came in on only. Tagless requests and requests relayed past one hop are ignored.
#[tokio::test]
async fn a_path_request_is_answered_once_on_its_own_interface() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x64; 64]));
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
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[9u8; 64]));
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
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[10u8; 64]));
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
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[11u8; 64]));
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
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[12u8; 64]));
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
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x66; 64]));
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
