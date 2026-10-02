//! Unanswered link requests expire (Ruling 28): lost requests cannot wedge the pending table.
//! They are reported as `LinkRequestTimedOut`, never as `LinkDown` (Ruling 46).

use retinue::Packet;
use retinue::announce::{AnnounceBlob, RAND_HASH_LEN};
use retinue::destination::DestinationName;
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::node::{
    Action, Actions, InterfaceId, InterruptionPermission, LINK_ESTABLISHMENT_TIMEOUT_PER_HOP,
    LINK_IDLE_TIMEOUT, Node, TransportConfig, link_request_timeout,
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
    let via_far = sent(&far.ingest(IFACE, &announce, 0));
    let via_near = sent(&near.ingest(IFACE, &via_far, 0));
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
