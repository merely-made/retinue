//! Link idle expiry, RTT reporting, keepalives and stale teardown.

use super::*;

/// A peer that vanishes sends no close; its slot still comes back after the idle timeout.
#[test]
fn a_silent_peer_releases_its_link_slot() {
    let (mut a, _b, _id) = linked();
    assert_eq!(a.link_count(), 1, "the link is up");

    // Nobody says anything for longer than the timeout, then the node's clock ticks.
    let later = LINK_IDLE_TIMEOUT + 1;
    let _ = a.poll(later, IFACE, Some(&blob([0x11; RAND_HASH_LEN])));

    assert_eq!(a.link_count(), 0, "a silent slot must come back");
    assert_eq!(
        a.expired_links(),
        1,
        "and be attributable, so a busy node reads differently from a deserted one",
    );
}

/// The other half: a link being used must not be reclaimed underneath it.
#[test]
fn a_link_that_keeps_talking_keeps_its_slot() {
    let (mut a, b, id) = linked();
    let keepalive = b
        .links
        .iter()
        .find(|(l, _, _)| l.id() == id)
        .map(|(l, _, _)| l.keepalive_packet(link::KEEPALIVE_RESPONSE))
        .unwrap();

    let mut now = 0;
    for _ in 0..4 {
        now += LINK_IDLE_TIMEOUT - 1;
        a.ingest(IFACE, &keepalive, now);
        let _ = a.poll(now, IFACE, Some(&blob([0x22; RAND_HASH_LEN])));
        assert_eq!(a.link_count(), 1, "a live peer keeps its slot at {now}");
    }
    assert_eq!(a.expired_links(), 0, "nothing reclaimed from a live peer");
}

fn liveness(node: &Node<32, 8, 4>, id: AddressHash) -> Liveness {
    node.links.iter().find(|(l, _, _)| l.id() == id).unwrap().2
}

fn sends(actions: Actions<8>) -> Vec<Packet> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            Action::Send { packet, .. } => Some(packet),
            _ => None,
        })
        .collect()
}

/// The initiator reports the RTT it measured from request to proof, and that is what
/// moves an RNS responder out of its handshake.
#[test]
fn the_initiator_reports_its_measured_rtt_on_link_up() {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(
        &a.open_link(b.destination(), IFACE, &[0x31; 64], 1_000)
            .unwrap(),
    )
    .unwrap();
    let proof = sent(&b.ingest(IFACE, &request, 1_100)).unwrap();
    assert_eq!(liveness(&b, link::link_id(&request).unwrap()).rtt(), None);
    let up = a.ingest(IFACE, &proof, 1_250);
    let id = link_up(&up).unwrap();
    let rtt = sent(&up).expect("an RTT packet goes out with the link");
    assert_eq!(rtt.context, link::CTX_LRRTT);
    assert_eq!(liveness(&a, id).rtt(), Some(250));
    assert_eq!(
        link_liveness::read_rtt(&a.links[0].0, &rtt),
        Some(250),
        "the packet carries the measurement",
    );
    b.ingest(IFACE, &rtt, 1_300);
    assert_eq!(
        liveness(&b, id).rtt(),
        Some(250),
        "max(own 200, reported 250)"
    );
}

/// An idle link stays up: the initiator's keepalive requests are answered by the
/// responder, and each side hears the other often enough never to go stale.
#[test]
fn an_idle_link_is_kept_alive_by_keepalives() {
    let (mut a, mut b, id) = linked();
    let (mut requests, mut responses) = (0, 0);
    for now in (1_000..=600_000).step_by(1_000) {
        for packet in sends(a.poll(now, IFACE, None)) {
            assert_eq!(packet.payload, [link::KEEPALIVE_REQUEST]);
            requests += 1;
            for answer in sends(b.ingest(IFACE, &packet, now)) {
                assert_eq!(answer.payload, [link::KEEPALIVE_RESPONSE]);
                responses += 1;
                assert!(sends(a.ingest(IFACE, &answer, now)).is_empty());
            }
        }
        assert!(
            sends(b.poll(now, IFACE, None)).is_empty(),
            "a responder never asks"
        );
    }
    assert!(a.has_link(id) && b.has_link(id));
    assert_eq!(requests, 120, "one per 5 s interval");
    assert_eq!(responses, 120);
}

/// A peer that vanishes is detected and torn down with a LINKCLOSE after two silent
/// keepalive intervals and the grace, instead of holding the slot for the idle timeout.
#[test]
fn a_vanished_peer_goes_stale_and_is_closed() {
    let (mut a, mut b, id) = linked();
    assert_eq!(sends(a.poll(5_000, IFACE, None)).len(), 1);
    let stale = a.poll(10_000, IFACE, None);
    assert!(!stale.iter().any(|x| matches!(x, Action::LinkDown { .. })));
    assert!(liveness(&a, id).is_stale());
    assert!(a.poll(14_999, IFACE, None).is_empty());
    let down = a.poll(15_000, IFACE, None);
    assert!(down.iter().any(|x| *x == Action::LinkDown { link_id: id }));
    let close = sent(&down).expect("a LINKCLOSE goes out");
    assert_eq!(close.context, link::CTX_LINKCLOSE);
    assert!(!a.has_link(id));
    assert_eq!(a.expired_links(), 1);
    // The far end, if it is still there, closes on it.
    let closed = b.ingest(IFACE, &close, 15_000);
    assert!(
        closed
            .iter()
            .any(|x| *x == Action::LinkDown { link_id: id })
    );
}

/// A responder whose initiator never reports an RTT drops the link at its handshake
/// deadline, silently, as RNS does.
#[test]
fn a_responder_without_an_rtt_drops_the_link_at_the_handshake_deadline() {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
    let _ = b.ingest(IFACE, &request, 0);
    let id = link::link_id(&request).unwrap();
    let deadline = link_liveness::handshake_timeout(0);
    assert_eq!(
        b.pause_assessment().earliest_link_expiry,
        Some(deadline),
        "a pause is checked against the handshake deadline",
    );
    assert!(b.poll(deadline - 1, IFACE, None).is_empty());
    let down = b.poll(deadline, IFACE, None);
    assert!(down.iter().any(|x| *x == Action::LinkDown { link_id: id }));
    assert!(
        sent(&down).is_none(),
        "no LINKCLOSE for a link that never activated"
    );
}

/// A shell that wakes at `next_deadline` keeps every timer: it is no later than a link's
/// keepalive or a transfer's watchdog, and polling at it always moves it on.
#[test]
fn next_deadline_covers_links_and_resources_and_moves_on() {
    let (mut a, _b, id) = linked();
    let _ = a.poll(0, IFACE, Some(&blob([0x33; RAND_HASH_LEN])));
    let keepalive = a.links[0].2.next_due();
    assert_eq!(a.next_deadline(), Some(keepalive), "the link's own timer");

    let started = a
        .publish(
            id,
            IFACE,
            &[7; 1_024],
            [0xEE; 4],
            &[7; crate::token::IV_LEN],
            0,
        )
        .unwrap();
    assert!(sent(&started).is_some());
    let watchdog = a.resource_deadline().expect("the advertisement waits");
    assert_eq!(a.next_deadline(), Some(watchdog.min(keepalive)));

    let mut wakes = 0;
    while let Some(at) = a.next_deadline() {
        let _ = a.poll(at, IFACE, None);
        assert!(a.next_deadline().is_none_or(|next| next > at));
        wakes += 1;
        assert!(wakes < 64, "bounded");
    }
    assert_eq!(a.link_count(), 0, "unanswered, the link ran down");
    assert!(!a.transfer_active(id));
}
