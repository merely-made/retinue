//! Relayed announces: the ingress interface, the retry, neighbour suppression and the cap.

use super::*;

use crate::node::{REBROADCAST_GRACE, REBROADCAST_WINDOW};
use crate::packet::HeaderType;

async fn next(iface: &mut Interface, within: Duration) -> Option<Packet> {
    tokio::time::timeout(within, iface.next_outbound())
        .await
        .ok()
        .flatten()
}

/// `packet` as a transport node `relay` would rebroadcast it: same signature, more hops.
fn relayed_by(packet: &Packet, relay: u8, hops: u8) -> Packet {
    let mut copy = packet.clone();
    copy.header_type = HeaderType::Type2;
    copy.transport = Some(AddressHash::from_bytes([relay; 16]));
    copy.hops = hops;
    copy
}

/// A single-radio repeater relays back out the radio it heard on, as RNS sends on every
/// interface, and retries once after the grace (`Transport.py` 765-829).
#[tokio::test(start_paused = true)]
async fn a_single_interface_endpoint_relays_on_its_ingress_and_retries_once() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x91; 64]));
    let mut radio = ep.attach_interface();
    ep.enable_routing();
    let (packet, announce) = peer_announce(0x92, "single-radio");
    let heard = tokio::time::Instant::now();
    assert!(radio.sink().deliver(packet));

    let first = next(&mut radio, Duration::from_secs(1))
        .await
        .expect("relayed");
    let first_at = tokio::time::Instant::now();
    assert!(first_at - heard <= Duration::from_millis(REBROADCAST_WINDOW));
    assert_eq!(first.destination, announce.destination);
    assert_eq!(
        (first.hops, first.transport),
        (1, Some(ep.identity().hash()))
    );

    let retry = next(&mut radio, Duration::from_secs(10))
        .await
        .expect("retried");
    assert_eq!(retry, first);
    let grace = Duration::from_millis(REBROADCAST_GRACE + REBROADCAST_WINDOW);
    assert!(tokio::time::Instant::now() - first_at >= grace);
    assert!(
        next(&mut radio, Duration::from_secs(30)).await.is_none(),
        "once"
    );
    assert_eq!(ep.routing_counters().forwarded_announces, 2);
}

/// Two neighbours heard relaying at our hop count, or one passing ours on, end the retry
/// (`Transport.py` 2183-2201).
#[tokio::test(start_paused = true)]
async fn heard_neighbour_rebroadcasts_cancel_our_retry() {
    for (copies, label) in [
        (vec![(0xB1, 1), (0xB2, 1)], "neighbours"),
        (vec![(0xB3, 2)], "onward"),
    ] {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x93; 64]));
        let mut radio = ep.attach_interface();
        ep.enable_routing();
        let (packet, _) = peer_announce(0x94, "suppressed");
        assert!(radio.sink().deliver(packet.clone()));
        assert!(next(&mut radio, Duration::from_secs(1)).await.is_some());

        for (relay, hops) in copies {
            assert!(radio.sink().deliver(relayed_by(&packet, relay, hops)));
        }
        assert!(
            next(&mut radio, Duration::from_secs(30)).await.is_none(),
            "{label}: no retry"
        );
        let counters = ep.routing_counters();
        assert_eq!(counters.suppressed_rebroadcasts, 1, "{label}");
        assert_eq!(counters.forwarded_announces, 1, "{label}");
    }
}

/// Where the interface's airtime is known, relayed announces keep to 2 % of it, queued rather
/// than dropped (`Transport.py` 1522-1585).
#[tokio::test(start_paused = true)]
async fn the_announce_cap_queues_rather_than_drops() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x95; 64]));
    let mut radio = ep.attach_interface();
    ep.enable_routing();
    ep.set_relay_jitter(Duration::ZERO);
    let airtime = crate::node::first_hop_airtime(62_500);
    ep.set_first_hop_airtime(radio.id(), Duration::from_millis(airtime));
    for seed in 0x96..0x99 {
        assert!(radio.sink().deliver(peer_announce(seed, "capped").0));
    }

    let mut last = None;
    for _ in 0..3 {
        next(&mut radio, Duration::from_secs(5))
            .await
            .expect("released");
        let now = tokio::time::Instant::now();
        if let Some(last) = last {
            assert!(
                now - last >= Duration::from_secs(1),
                "about 1 s of budget each"
            );
        }
        last = Some(now);
    }
    let counters = ep.routing_counters();
    assert_eq!(counters.capped_announces, 2);
    assert_eq!(counters.dropped_announces, 0);
    assert_eq!(counters.forwarded_announces, 3);
}
