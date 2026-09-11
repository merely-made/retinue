use retinue::Packet;
use retinue::announce::RAND_HASH_LEN;
use retinue::destination::DestinationName;
use retinue::identity::PrivateIdentity;
use retinue::node::{Action, Actions, InterfaceId, LINK_IDLE_TIMEOUT, Node, PauseBlocked};

const IFACE: InterfaceId = 0;
type TestNode = Node<32, 8, 4>;

fn pair() -> (TestNode, TestNode) {
    (
        Node::new(
            PrivateIdentity::from_secret_bytes(&[0x11; 64]),
            DestinationName::new("retinue", ["pause-a"]).name_hash(),
        ),
        Node::new(
            PrivateIdentity::from_secret_bytes(&[0x22; 64]),
            DestinationName::new("retinue", ["pause-b"]).name_hash(),
        ),
    )
}

fn sent<const N: usize>(actions: &Actions<N>) -> Packet {
    assert_eq!(actions.overflowed(), 0, "packet actions overflowed");
    actions
        .iter()
        .find_map(|action| match action {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        })
        .expect("action should contain a packet")
}

fn sent_context<const N: usize>(actions: &Actions<N>, context: u8) -> Packet {
    actions
        .iter()
        .find_map(|action| match action {
            Action::Send { packet, .. } if packet.context == context => Some(packet.clone()),
            _ => None,
        })
        .expect("action should contain the requested resource packet")
}

fn link_up<const N: usize>(actions: &Actions<N>) -> retinue::hash::AddressHash {
    actions
        .iter()
        .find_map(|action| match action {
            Action::LinkUp { link_id } => Some(*link_id),
            _ => None,
        })
        .expect("link should be established")
}

fn linked() -> (TestNode, TestNode, retinue::hash::AddressHash) {
    linked_at(0)
}

fn linked_at(seen: u64) -> (TestNode, TestNode, retinue::hash::AddressHash) {
    let (mut a, mut b) = pair();
    let announce = b.announce(
        &retinue::announce::AnnounceBlob::from_wire([2; RAND_HASH_LEN]),
        None,
    );
    a.ingest(IFACE, &announce, seen);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64]).unwrap());
    let proof = sent(&b.ingest(IFACE, &request, seen));
    let id = link_up(&a.ingest(IFACE, &proof, seen));
    (a, b, id)
}

#[test]
fn established_link_can_pause_and_encrypted_data_survives_resume() {
    let (a, mut b, id) = linked();
    let assessment = a.pause_assessment();
    assert!(assessment.earliest_link_expiry.is_some());
    assessment
        .can_pause_through(0, LINK_IDLE_TIMEOUT - 1)
        .unwrap();

    let outbound = a.send(id, IFACE, b"after-resume", &[0x41; 16]).unwrap();
    let frame = sent(&outbound);
    let actions = b.ingest(IFACE, &frame, 1);
    assert!(actions.iter().any(|action| matches!(action, Action::Data { link_id, payload } if *link_id == id && payload == b"after-resume")));
}

#[test]
fn link_expiry_is_checked_against_elapsed_return_bound() {
    let (a, _b, _id) = linked();
    let assessment = a.pause_assessment();
    assert!(matches!(
        assessment.can_pause_through(0, LINK_IDLE_TIMEOUT),
        Err(PauseBlocked::LinkExpiresAt { .. })
    ));
    assert!(matches!(
        assessment.can_pause_through(100, LINK_IDLE_TIMEOUT),
        Err(PauseBlocked::LinkExpiresAt { .. })
    ));
}

#[test]
fn poll_reclaims_expired_link_before_a_later_pause_query() {
    let (mut a, _b, _id) = linked();
    assert_eq!(a.link_count(), 1);
    let _ = a.poll(LINK_IDLE_TIMEOUT, IFACE, None);
    assert_eq!(a.link_count(), 0);
    a.pause_assessment()
        .can_pause_through(LINK_IDLE_TIMEOUT, LINK_IDLE_TIMEOUT + 1)
        .unwrap();
}

#[test]
fn pending_handshake_blocks_then_clears_after_proof() {
    let (mut a, mut b) = pair();
    a.ingest(
        IFACE,
        &b.announce(
            &retinue::announce::AnnounceBlob::from_wire([3; RAND_HASH_LEN]),
            None,
        ),
        0,
    );
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x52; 64]).unwrap());
    let assessment = a.pause_assessment();
    assert_eq!(assessment.pending_handshakes, 1);
    assert!(matches!(
        assessment.can_pause_through(0, 100),
        Err(PauseBlocked::PendingHandshakes { count: 1 })
    ));
    let proof = sent(&b.ingest(IFACE, &request, 1));
    a.ingest(IFACE, &proof, 2);
    assert_eq!(a.pause_assessment().pending_handshakes, 0);
}

#[test]
fn active_resource_blocks_pause_without_mutating_transfer() {
    let (mut a, _b, id) = linked();
    let sender = a
        .publish(id, IFACE, &[0x77; 64], [0x33; 4], &[0x44; 16], 0)
        .unwrap();
    assert!(
        sender
            .iter()
            .any(|action| matches!(action, Action::Send { .. }))
    );
    let before = a.pause_assessment();
    assert_eq!(before.outbound_resources, 1);
    assert!(matches!(
        before.can_pause_through(0, 100),
        Err(PauseBlocked::ActiveResources { outbound: 1, .. })
    ));
    let after = a.pause_assessment();
    assert_eq!(after, before);
    assert!(a.transfer_active(id));
}

#[test]
fn return_bound_before_now_is_rejected() {
    let (a, _b, _id) = linked();
    assert!(matches!(
        a.pause_assessment().can_pause_through(50, 49),
        Err(PauseBlocked::ReturnBoundBeforeNow {
            now: 50,
            return_by: 49
        })
    ));
}

fn pump_resource(
    a: &mut TestNode,
    b: &mut TestNode,
    first: Packet,
    id: retinue::hash::AddressHash,
    drop_proof: bool,
) -> (Vec<Vec<u8>>, bool, bool) {
    let mut queue: Vec<(bool, Packet)> = vec![(false, first)];
    let mut received = Vec::new();
    let mut sender_busy_when_received = false;
    let mut dropped_proof = false;
    for _ in 0..128 {
        if queue.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for (to_b, packet) in queue.drain(..) {
            if drop_proof && !to_b && packet.context == retinue::link::CTX_RESOURCE_PRF {
                dropped_proof = true;
                continue;
            }
            let actions = if to_b {
                b.ingest(IFACE, &packet, 1)
            } else {
                a.ingest(IFACE, &packet, 1)
            };
            assert_eq!(actions.overflowed(), 0, "resource actions overflowed");
            for action in actions {
                match action {
                    Action::Send { interface, packet } => {
                        assert_eq!(interface, IFACE);
                        next.push((!to_b, packet));
                    }
                    Action::Resource { link_id, data } => {
                        assert!(to_b, "only the receiver may deliver this resource");
                        assert_eq!(link_id, id);
                        received.push(data);
                        sender_busy_when_received |= a.transfer_active(id);
                    }
                    Action::LinkUp { .. } | Action::LinkDown { .. } => {
                        panic!("link lifecycle action during established resource")
                    }
                    Action::Learned { .. } | Action::Data { .. } => {
                        panic!("unexpected non-resource action during resource pump")
                    }
                }
            }
        }
        queue = next;
    }
    assert!(
        queue.is_empty(),
        "resource action queue must drain within bound"
    );
    (received, sender_busy_when_received, dropped_proof)
}

#[test]
fn multipart_resource_blocks_then_completion_allows_pause() {
    let (mut a, mut b, id) = linked();
    let payload: Vec<u8> = (0..3_000u32).map(|n| (n.wrapping_mul(17)) as u8).collect();
    let started = a
        .publish(id, IFACE, &payload, [0x91; 4], &[0x92; 16], 0)
        .unwrap();
    assert!(matches!(
        a.pause_assessment().can_pause_through(0, 100),
        Err(PauseBlocked::ActiveResources { outbound: 1, .. })
    ));
    let advertisement = sent(&started);
    let offer = b.ingest(IFACE, &advertisement, 1);
    assert_eq!(b.pause_assessment().inbound_resources, 1);
    assert!(matches!(
        b.pause_assessment().can_pause_through(1, 100),
        Err(PauseBlocked::ActiveResources { inbound: 1, .. })
    ));
    let request = sent(&offer);
    let (received, sender_busy_when_received, dropped_proof) =
        pump_resource(&mut a, &mut b, request, id, false);
    assert_eq!(received, vec![payload]);
    assert!(
        sender_busy_when_received,
        "delivery precedes final sender proof"
    );
    assert!(!dropped_proof);
    assert!(!a.transfer_active(id));
    assert!(!b.transfer_active(id));
    a.pause_assessment().can_pause_through(1, 100).unwrap();
    b.pause_assessment().can_pause_through(1, 100).unwrap();
    let data = sent(&a.send(id, IFACE, b"after-resource", &[0x93; 16]).unwrap());
    assert!(b.ingest(IFACE, &data, 2).iter().any(
        |action| matches!(action, Action::Data { payload, .. } if payload == b"after-resource")
    ));
}

#[test]
fn dropped_final_proof_keeps_sender_busy_and_poll_retries() {
    let (mut a, mut b, id) = linked();
    let payload = vec![0xA5; 1_024];
    let started = a
        .publish(id, IFACE, &payload, [0xA7; 4], &[0xA8; 16], 0)
        .unwrap();
    let advertisement = sent(&started);
    let offer = b.ingest(IFACE, &advertisement, 1);
    let request = sent(&offer);
    let (received, busy_at_delivery, dropped) = pump_resource(&mut a, &mut b, request, id, true);
    assert_eq!(received, vec![payload]);
    assert!(busy_at_delivery);
    assert!(dropped);
    assert!(a.transfer_active(id));
    assert!(matches!(
        a.pause_assessment().can_pause_through(1, 100),
        Err(PauseBlocked::ActiveResources { outbound: 1, .. })
    ));

    let retry = a.poll(retinue::node::RESOURCE_RETRY_INTERVAL + 1, IFACE, None);
    assert_eq!(retry.overflowed(), 0);
    let retry_packet = sent_context(&retry, retinue::link::CTX_RESOURCE_ADV);
    assert_eq!(retry_packet.context, retinue::link::CTX_RESOURCE_ADV);
    assert!(a.transfer_active(id));
    assert!(matches!(
        a.pause_assessment().can_pause_through(
            retinue::node::RESOURCE_RETRY_INTERVAL + 1,
            retinue::node::RESOURCE_RETRY_INTERVAL + 100
        ),
        Err(PauseBlocked::ActiveResources { outbound: 1, .. })
    ));
}

#[test]
fn regressed_clock_before_latest_link_activity_is_rejected() {
    let (a, _b, _id) = linked_at(1_000);
    let assessment = a.pause_assessment();
    assert!(matches!(
        assessment.can_pause_through(999, 1_000),
        Err(PauseBlocked::ClockBeforeLinkActivity {
            now: 999,
            latest_activity: 1_000
        })
    ));
}

#[test]
fn inbound_resource_offer_blocks_pause() {
    let (mut a, mut b, id) = linked();
    let offer = sent(
        &b.publish(id, IFACE, &[0x88; 64], [0x55; 4], &[0x66; 16], 0)
            .unwrap(),
    );
    let actions = a.ingest(IFACE, &offer, 1);
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, Action::Send { .. }))
    );
    let assessment = a.pause_assessment();
    assert_eq!(assessment.inbound_resources, 1);
    assert!(matches!(
        assessment.can_pause_through(1, 100),
        Err(PauseBlocked::ActiveResources { inbound: 1, .. })
    ));
}

#[test]
fn link_expiry_overflow_fails_closed() {
    let (a, _b, _id) = linked_at(u64::MAX - 100);
    let assessment = a.pause_assessment();
    assert!(assessment.link_expiry_overflow);
    assert!(matches!(
        assessment.can_pause_through(u64::MAX - 100, u64::MAX),
        Err(PauseBlocked::LinkExpiryOverflow)
    ));
}
