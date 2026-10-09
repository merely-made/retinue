//! Unanswered link requests expire (Ruling 28): lost requests cannot wedge the pending table.
//! They are reported as `LinkRequestTimedOut`, never as `LinkDown` (Ruling 46).

use retinue::Packet;
use retinue::announce::{AnnounceBlob, RAND_HASH_LEN};
use retinue::destination::DestinationName;
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::node::{
    Action, Actions, AirtimeTableFull, FIRST_HOP_AIRTIME_INTERFACES, FIRST_HOP_ALLOWANCE_BITS,
    InterfaceId, InterruptionPermission, LINK_ESTABLISHMENT_TIMEOUT_PER_HOP, LINK_IDLE_TIMEOUT,
    Node, TransportConfig, first_hop_airtime, link_request_timeout,
};

const IFACE: InterfaceId = 0;
/// The T114 channel node's bounds: four pending links.
type Board = Node<32, 8, 4, 16>;

fn node(seed: u8, name: &str) -> Board {
    Node::new(
        PrivateIdentity::from_secret_bytes(&[seed; 64]),
        DestinationName::new("retinue", [name]).name_hash(),
    )
}

fn blob(byte: u8) -> AnnounceBlob {
    AnnounceBlob::from_wire([byte; RAND_HASH_LEN])
}

fn sent<const N: usize>(actions: &Actions<N>) -> Packet {
    actions
        .iter()
        .find_map(|action| match action {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        })
        .expect("a packet to send")
}

/// A transport node's rebroadcast of `announce`, from the poll it falls due in.
fn relayed(relay: &mut Board, announce: &Packet, now: u64) -> Packet {
    relay.ingest(IFACE, announce, now);
    sent(&relay.poll(relay.next_rebroadcast().expect("scheduled"), IFACE, None))
}

fn links_down<const N: usize>(actions: &Actions<N>) -> Vec<AddressHash> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::LinkDown { link_id } => Some(*link_id),
            _ => None,
        })
        .collect()
}

fn timed_out<const N: usize>(actions: &Actions<N>) -> Vec<AddressHash> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::LinkRequestTimedOut { link_id } => Some(*link_id),
            _ => None,
        })
        .collect()
}

/// Open a link whose request is never delivered, returning its link id.
fn lose_a_request(sender: &mut Board, to: AddressHash, seed: u8, now: u64) -> AddressHash {
    let request = sent(&sender.open_link(to, IFACE, &[seed; 64], now).unwrap());
    retinue::link::link_id(&request).unwrap()
}

/// The S7 wedge: four lost requests fill the board's table. Before the deadline the wedge
/// holds (the control); at it, poll reports each request timed out and a new link comes up.
#[test]
fn lost_requests_wedge_the_table_until_their_deadline_then_free_it() {
    let mut sender = node(0x11, "sender");
    let mut peer = node(0x22, "peer");
    sender.ingest(IFACE, &peer.announce(&blob(2), None), 0);
    let to = peer.destination();

    let lost: Vec<_> = (0..4)
        .map(|i| lose_a_request(&mut sender, to, 0x31 + i, 0))
        .collect();
    assert_eq!(sender.pause_assessment().pending_handshakes, 4);
    assert!(sender.open_link(to, IFACE, &[0x40; 64], 0).is_none());
    assert_eq!(sender.refused_links(), 1, "the full table refuses");

    // A direct peer: no route, no relays.
    let deadline = link_request_timeout(0);
    assert_eq!(deadline, 2 * LINK_ESTABLISHMENT_TIMEOUT_PER_HOP);

    // Control: one tick short of the deadline nothing expires and the wedge holds.
    let before = sender.poll(deadline - 1, IFACE, None);
    assert!(timed_out(&before).is_empty());
    assert!(
        sender
            .open_link(to, IFACE, &[0x41; 64], deadline - 1)
            .is_none()
    );
    assert_eq!(sender.refused_links(), 2);
    assert_eq!(sender.expired_link_requests(), 0);

    // At the deadline every lost request is reported timed out, by the id its request
    // named. None of them is reported as a link going down: none came up.
    let at = sender.poll(deadline, IFACE, None);
    assert_eq!(timed_out(&at), lost);
    assert!(links_down(&at).is_empty());
    assert_eq!(sender.pause_assessment().pending_handshakes, 0);
    assert_eq!(sender.expired_link_requests(), 4);

    // The freed slot carries a real link.
    let request = sent(&sender.open_link(to, IFACE, &[0x42; 64], deadline).unwrap());
    let proof = sent(&peer.ingest(IFACE, &request, deadline));
    let up = sender.ingest(IFACE, &proof, deadline);
    assert!(
        up.iter()
            .any(|action| matches!(action, Action::LinkUp { .. }))
    );
    assert_eq!(sender.link_count(), 1);
}

/// The deadline grows with the route: two relays away is 24 s, against 12 s direct. A proof
/// arriving after the request expired does not bring the link up.
#[test]
fn the_deadline_scales_with_relays_and_a_late_proof_is_ignored() {
    let mut source = node(0x11, "source");
    let mut destination = node(0x22, "destination");
    let transit = |seed, name| node(seed, name).with_transport_config(TransportConfig::transit());
    // source - near - far - destination
    let mut near = transit(0x48, "near");
    let mut far = transit(0x49, "far");
    let announce = destination.announce(&blob(0x79), None);
    let via_far = relayed(&mut far, &announce, 0);
    let via_near = relayed(&mut near, &via_far, 0);
    source.ingest(IFACE, &via_near, 0);
    let to = destination.destination();
    assert_eq!(source.next_hop(to, 0).unwrap().hops, 2);

    let deadline = link_request_timeout(2);
    assert_eq!(deadline, 4 * LINK_ESTABLISHMENT_TIMEOUT_PER_HOP);
    let request = sent(&source.open_link(to, IFACE, &[0x9B; 64], 0).unwrap());
    let id = retinue::link::link_id(&request).unwrap();

    // Still pending where a direct request would already have expired.
    assert!(timed_out(&source.poll(link_request_timeout(0), IFACE, None)).is_empty());
    assert!(timed_out(&source.poll(deadline - 1, IFACE, None)).is_empty());
    assert_eq!(timed_out(&source.poll(deadline, IFACE, None)), vec![id]);

    // The request did travel; its proof comes back too late.
    let at_far = sent(&near.ingest(IFACE, &request, 1));
    let at_destination = sent(&far.ingest(IFACE, &at_far, 2));
    let proof = sent(&destination.ingest(IFACE, &at_destination, 3));
    let proof = sent(&far.ingest(IFACE, &proof, 4));
    let proof = sent(&near.ingest(IFACE, &proof, 5));
    let late = source.ingest(IFACE, &proof, deadline);
    assert!(
        !late
            .iter()
            .any(|action| matches!(action, Action::LinkUp { .. }))
    );
    assert_eq!(source.link_count(), 0);
}

/// `expire_sessions` reports expired requests itself, so a resident caller that reconciles
/// before polling sees them once, and the poll after it repeats nothing.
#[test]
fn expire_sessions_reports_expired_requests_once() {
    let mut sender = node(0x11, "sender");
    let peer = node(0x22, "peer");
    sender.ingest(IFACE, &peer.announce(&blob(2), None), 0);
    let id = lose_a_request(&mut sender, peer.destination(), 0x31, 0);
    let deadline = link_request_timeout(0);

    assert!(
        sender
            .expire_sessions(deadline - 1)
            .pending_links
            .is_empty()
    );
    let report = sender.expire_sessions(deadline);
    assert_eq!(report.pending_links.as_slice(), &[id]);
    assert!(report.links.is_empty());
    let after = sender.poll(deadline, IFACE, None);
    assert!(timed_out(&after).is_empty());
    assert!(links_down(&after).is_empty());
}

/// The other half of Ruling 46: a link that came up and then ended is still `LinkDown`,
/// whether the peer closed it or it went idle, and never `LinkRequestTimedOut`.
#[test]
fn an_established_link_that_ends_is_still_link_down() {
    let mut sender = node(0x11, "sender");
    let mut quiet_peer = node(0x22, "quiet");
    let mut closing_peer = node(0x23, "closing");
    sender.ingest(IFACE, &quiet_peer.announce(&blob(2), None), 0);
    sender.ingest(IFACE, &closing_peer.announce(&blob(3), None), 0);
    let mut link = |peer: &mut Board, seed: u8| {
        let request = sent(
            &sender
                .open_link(peer.destination(), IFACE, &[seed; 64], 0)
                .unwrap(),
        );
        let proof = sent(&peer.ingest(IFACE, &request, 0));
        let up = sender.ingest(IFACE, &proof, 0);
        up.iter()
            .find_map(|action| match action {
                Action::LinkUp { link_id } => Some(*link_id),
                _ => None,
            })
            .expect("link up")
    };
    let idle = link(&mut quiet_peer, 0x51);
    let closed = link(&mut closing_peer, 0x52);

    // Past every request deadline, an established link is not a timed-out request.
    let quiet = sender.poll(link_request_timeout(3) + 1, IFACE, None);
    assert!(timed_out(&quiet).is_empty());
    assert!(links_down(&quiet).is_empty());
    assert_eq!(sender.link_count(), 2);

    // One peer closes its link.
    let report = closing_peer
        .force_interrupt(InterruptionPermission::AllowSessionLoss, || [0x61; 16])
        .unwrap();
    let down = sender.ingest(IFACE, &report.close_packets[0], 1);
    assert_eq!(links_down(&down), vec![closed]);
    assert!(timed_out(&down).is_empty());

    // The other goes idle and is reclaimed.
    let expired = sender.poll(LINK_IDLE_TIMEOUT, IFACE, None);
    assert_eq!(links_down(&expired), vec![idle]);
    assert!(timed_out(&expired).is_empty());
    assert_eq!(sender.expired_link_requests(), 0);
}

/// Ruling 54: `open_link` drops expired requests itself, so a caller that has not polled is
/// not refused for a table full of requests past their deadline. The expiries come back
/// first, as `poll` would have reported them, and a later poll repeats none of them.
#[test]
fn open_link_expires_the_table_without_a_poll() {
    let mut sender = node(0x11, "sender");
    let mut peer = node(0x22, "peer");
    sender.ingest(IFACE, &peer.announce(&blob(2), None), 0);
    let to = peer.destination();
    let lost: Vec<_> = (0..4)
        .map(|i| lose_a_request(&mut sender, to, 0x31 + i, 0))
        .collect();
    let deadline = link_request_timeout(0);

    // Control: one tick short of the deadline the full table still refuses, and drops none.
    assert!(
        sender
            .open_link(to, IFACE, &[0x40; 64], deadline - 1)
            .is_none()
    );
    assert_eq!(sender.refused_links(), 1);
    assert_eq!(sender.pause_assessment().pending_handshakes, 4);

    // At the deadline, with no poll in between, the request goes out.
    let opened = sender
        .open_link(to, IFACE, &[0x41; 64], deadline)
        .expect("expired requests no longer hold the table");
    assert_eq!(timed_out(&opened), lost);
    assert!(links_down(&opened).is_empty());
    let kinds: Vec<_> = opened
        .iter()
        .map(|action| matches!(action, Action::Send { .. }))
        .collect();
    assert_eq!(kinds, [false, false, false, false, true], "expiries first");
    assert_eq!(sender.refused_links(), 1, "not refused this time");
    assert_eq!(sender.expired_link_requests(), 4);
    assert_eq!(sender.pause_assessment().pending_handshakes, 1);

    // Nothing is reported twice, and the new request is a working one.
    assert!(timed_out(&sender.poll(deadline, IFACE, None)).is_empty());
    let proof = sent(&peer.ingest(IFACE, &sent(&opened), deadline));
    let up = sender.ingest(IFACE, &proof, deadline);
    assert!(
        up.iter()
            .any(|action| matches!(action, Action::LinkUp { .. }))
    );
}

/// Ruling 50: the caller's first-hop airtime allowance moves the deadline by exactly its
/// amount, only for requests leaving by that interface, and zero gives the base deadline.
#[test]
fn the_first_hop_allowance_moves_the_deadline_by_exactly_its_amount() {
    // 62,500 bps is the bitrate V1 measured RNS against: 12.064 s computed, 12.079 s seen.
    assert_eq!(first_hop_airtime(62_500), 64);
    assert_eq!(link_request_timeout(0) + first_hop_airtime(62_500), 12_064);
    assert_eq!(first_hop_airtime(0), 0, "unbounded, as TCP");
    assert_eq!(FIRST_HOP_ALLOWANCE_BITS, 4_000);

    const OTHER: InterfaceId = 7;
    // The deadline a request opened at 0 by `interface` expires at, found by polling.
    let expiry = |allowances: &[(InterfaceId, u64)], interface: InterfaceId| {
        let mut sender = node(0x11, "sender");
        let peer = node(0x22, "peer");
        sender.ingest(IFACE, &peer.announce(&blob(2), None), 0);
        for (id, allowance) in allowances {
            sender.set_first_hop_airtime(*id, *allowance).unwrap();
        }
        let request = sent(
            &sender
                .open_link(peer.destination(), interface, &[0x31; 64], 0)
                .unwrap(),
        );
        let id = retinue::link::link_id(&request).unwrap();
        let base = link_request_timeout(0);
        let mut t = base - 1;
        loop {
            if timed_out(&sender.poll(t, IFACE, None)) == vec![id] {
                return t;
            }
            t += 1;
            assert!(t <= base + 10_000, "never expired");
        }
    };

    let base = link_request_timeout(0);
    assert_eq!(expiry(&[], IFACE), base, "no allowance: today's deadline");
    assert_eq!(expiry(&[(IFACE, 0)], IFACE), base, "zero: today's deadline");
    assert_eq!(expiry(&[(IFACE, 64)], IFACE), base + 64);
    assert_eq!(expiry(&[(IFACE, 3_724)], IFACE), base + 3_724);
    assert_eq!(
        expiry(&[(OTHER, 3_724)], IFACE),
        base,
        "another interface's"
    );
    assert_eq!(expiry(&[(IFACE, 64), (OTHER, 3_724)], OTHER), base + 3_724);
    assert_eq!(expiry(&[(IFACE, 64), (IFACE, 0)], IFACE), base, "cleared");
}

#[test]
fn the_allowance_table_is_bounded_and_says_so() {
    let mut sender = node(0x11, "sender");
    for interface in 0..FIRST_HOP_AIRTIME_INTERFACES as InterfaceId {
        sender.set_first_hop_airtime(interface, 10).unwrap();
    }
    let full = FIRST_HOP_AIRTIME_INTERFACES as InterfaceId;
    assert_eq!(
        sender.set_first_hop_airtime(full, 10),
        Err(AirtimeTableFull)
    );
    assert_eq!(sender.first_hop_airtime(full), 0);
    // Updating or clearing a held interface still works, and clearing frees a slot.
    sender.set_first_hop_airtime(0, 20).unwrap();
    assert_eq!(sender.first_hop_airtime(0), 20);
    sender.set_first_hop_airtime(0, 0).unwrap();
    sender.set_first_hop_airtime(full, 10).unwrap();
    assert_eq!(sender.first_hop_airtime(full), 10);
}

/// An unknown destination is refused before anything is expired, so the expiry is not lost
/// with the refusal: the next poll still reports it.
#[test]
fn a_refused_open_link_does_not_swallow_an_expiry() {
    let mut sender = node(0x11, "sender");
    let peer = node(0x22, "peer");
    sender.ingest(IFACE, &peer.announce(&blob(2), None), 0);
    let id = lose_a_request(&mut sender, peer.destination(), 0x31, 0);
    let deadline = link_request_timeout(0);
    let stranger = node(0x33, "stranger").destination();

    assert!(
        sender
            .open_link(stranger, IFACE, &[0x41; 64], deadline)
            .is_none()
    );
    assert_eq!(timed_out(&sender.poll(deadline, IFACE, None)), vec![id]);
}
