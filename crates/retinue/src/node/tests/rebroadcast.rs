//! Relayed announces: jitter, the retry, neighbour suppression and the announce cap.

use super::*;

fn transit(seed: u8, name: &str) -> Node<32, 8, 4, 4> {
    Node::new(
        PrivateIdentity::from_secret_bytes(&[seed; 64]),
        DestinationName::new("retinue", [name]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit())
}

fn announcer(seed: u8) -> Node<32, 8, 4> {
    Node::new(
        PrivateIdentity::from_secret_bytes(&[seed; 64]),
        DestinationName::new("retinue", ["announcer"]).name_hash(),
    )
}

const RETRY: u64 = REBROADCAST_GRACE + REBROADCAST_WINDOW;

/// RNS holds a relayed announce for a random 0-0.5 s, sends it on every interface including
/// the one it came from, and tries once more after the 5 s grace (`Transport.py` 765-829).
#[test]
fn a_relay_sends_after_a_jitter_on_the_ingress_interface_then_retries_once() {
    let destination = announcer(0x71);
    let mut relay = transit(0x61, "relay");
    let announce = destination.announce(&blob([0x71; RAND_HASH_LEN]), None);

    assert!(sent(&relay.ingest(IFACE + 1, &announce, 1_000)).is_none());
    let due = relay.next_rebroadcast().expect("scheduled");
    assert!((1_000..=1_000 + REBROADCAST_WINDOW).contains(&due));
    if due > 1_000 {
        assert!(relay.poll(due - 1, IFACE, None).is_empty());
    }
    let first = relay.poll(due, IFACE, None);
    let Some(Action::Send { interface, packet }) = first.iter().next() else {
        panic!("the rebroadcast is due");
    };
    assert_eq!(
        *interface,
        IFACE + 1,
        "back out the interface it was heard on"
    );
    assert_eq!(packet.hops, 1);
    assert_eq!(packet.transport, Some(relay.identity.hash()));

    assert_eq!(relay.next_rebroadcast(), Some(due + RETRY));
    assert!(relay.poll(due + RETRY - 1, IFACE, None).is_empty());
    assert_eq!(
        sent(&relay.poll(due + RETRY, IFACE, None)).as_ref(),
        Some(packet)
    );
    assert_eq!(relay.next_rebroadcast(), None, "one retry only");
    assert_eq!(relay.transport_counters().forwarded_announces, 2);
}

/// Two neighbours heard relaying the announce at our hop count end our retry
/// (`Transport.py` 2183-2194). Before our first send they only count, as in RNS.
#[test]
fn heard_neighbour_rebroadcasts_cancel_our_retry() {
    let destination = announcer(0x72);
    let mut relay = transit(0x62, "relay");
    let announce = destination.announce(&blob([0x72; RAND_HASH_LEN]), None);
    let first_neighbour = relayed(&mut transit(0x63, "n1"), IFACE, &announce, 0).unwrap();
    let second_neighbour = relayed(&mut transit(0x64, "n2"), IFACE, &announce, 0).unwrap();

    relay.ingest(IFACE, &announce, 0);
    let due = relay.next_rebroadcast().unwrap();
    relay.ingest(IFACE, &first_neighbour, due);
    assert!(
        sent(&relay.poll(due, IFACE, None)).is_some(),
        "a neighbour heard before our first send does not cancel it"
    );

    relay.ingest(IFACE, &second_neighbour, due + 1);
    assert_eq!(
        relay.next_rebroadcast(),
        None,
        "the second neighbour ends it"
    );
    assert!(relay.poll(due + RETRY, IFACE, None).is_empty());
    let counters = relay.transport_counters();
    assert_eq!(counters.suppressed_rebroadcasts, 1);
    assert_eq!(counters.forwarded_announces, 1);
}

/// Our rebroadcast heard passed on with one more hop ends the retry (`Transport.py`
/// 2196-2201).
#[test]
fn our_rebroadcast_passed_on_cancels_the_retry() {
    let destination = announcer(0x73);
    let mut relay = transit(0x65, "relay");
    let announce = destination.announce(&blob([0x73; RAND_HASH_LEN]), None);
    let ours = relayed(&mut relay, IFACE, &announce, 0).unwrap();
    let onward = relayed(&mut transit(0x66, "downstream"), IFACE, &ours, 1).unwrap();
    assert_eq!(onward.hops, ours.hops + 1);

    assert!(relay.next_rebroadcast().is_some());
    relay.ingest(IFACE, &onward, 600);
    assert_eq!(relay.next_rebroadcast(), None);
    assert_eq!(relay.transport_counters().suppressed_rebroadcasts, 1);
}

/// With the interface's airtime known, relayed announces keep to 2 % of it: the rest wait in
/// the queue rather than being dropped (`Transport.py` 1522-1585).
#[test]
fn the_announce_cap_queues_rather_than_drops() {
    let mut relay = transit(0x67, "relay");
    relay
        .set_first_hop_airtime(IFACE, first_hop_airtime(62_500))
        .unwrap();
    for seed in 0x74..0x77 {
        let announce = announcer(seed).announce(&blob([seed; RAND_HASH_LEN]), None);
        relay.ingest(IFACE, &announce, 0);
    }

    assert_eq!(relay.poll(REBROADCAST_WINDOW, IFACE, None).len(), 1);
    assert_eq!(relay.transport_counters().capped_announces, 2);
    let mut at = REBROADCAST_WINDOW;
    for _ in 0..2 {
        let next = relay.next_rebroadcast().unwrap();
        assert!(
            next > at + 1_000,
            "about a second of budget per 2 % announce"
        );
        assert!(relay.poll(next - 1, IFACE, None).is_empty());
        assert_eq!(relay.poll(next, IFACE, None).len(), 1);
        at = next;
    }
    let counters = relay.transport_counters();
    assert_eq!(counters.forwarded_announces, 3);
    assert_eq!(counters.dropped_announces, 0);
}

/// A flood cannot displace announces still waiting for their first send.
#[test]
fn a_full_rebroadcast_table_refuses_rather_than_displaces() {
    let mut relay = transit(0x68, "relay");
    for seed in 0..=REBROADCAST_SLOTS as u8 {
        let announce = announcer(0x80 + seed).announce(&blob([seed; RAND_HASH_LEN]), None);
        relay.ingest(IFACE, &announce, 0);
    }
    assert_eq!(relay.transport_counters().refused_rebroadcasts, 1);
    assert_eq!(
        relay.poll(REBROADCAST_WINDOW, IFACE, None).len(),
        REBROADCAST_SLOTS
    );
}

/// A destination re-announcing while its relay waits for the retry takes over that retry, so
/// repeats of a few destinations cannot keep the bounded table full of unsent entries.
#[test]
fn re_announcing_cannot_pin_the_rebroadcast_table() {
    let mut relay = transit(0x69, "relay");
    let announce = |seed: u8, ordinal| {
        let blob = AnnounceBlob::mint([seed; 5], ordinal).unwrap();
        announcer(0x90 + seed).announce(&blob, None)
    };
    for seed in 0..REBROADCAST_SLOTS as u8 {
        relay.ingest(IFACE, &announce(seed, 1), 0);
    }
    let first = relay.poll(REBROADCAST_WINDOW, IFACE, None);
    assert_eq!(first.len(), REBROADCAST_SLOTS);
    for seed in 0..REBROADCAST_SLOTS as u8 {
        relay.ingest(IFACE, &announce(seed, 2), REBROADCAST_WINDOW + 1);
    }

    let newcomer = announcer(0x9F).announce(&blob([0x9F; RAND_HASH_LEN]), None);
    relay.ingest(IFACE, &newcomer, REBROADCAST_WINDOW + 2);
    assert_eq!(relay.transport_counters().refused_rebroadcasts, 0);
}

/// A shell that wakes at `next_rebroadcast` and polls always makes progress: the next wake is
/// later, or there is none, so a deadline-driven board cannot spin.
#[test]
fn polling_at_the_next_rebroadcast_moves_it_on() {
    let mut relay = transit(0x6A, "relay");
    relay
        .set_first_hop_airtime(IFACE, first_hop_airtime(62_500))
        .unwrap();
    for seed in 0xA0..0xA6 {
        let announce = announcer(seed).announce(&blob([seed; RAND_HASH_LEN]), None);
        relay.ingest(IFACE, &announce, 0);
    }
    let mut wakes = 0;
    while let Some(at) = relay.next_rebroadcast() {
        relay.poll(at, IFACE, None);
        assert!(relay.next_rebroadcast().is_none_or(|next| next > at));
        wakes += 1;
        assert!(wakes < 64, "bounded");
    }
    let counters = relay.transport_counters();
    assert!(counters.forwarded_announces >= 6, "each sent at least once");
    assert_eq!(counters.dropped_announces, 0);
}
