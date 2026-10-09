//! Node pause assessment, forced interruption, and payload budgets.

use retinue::Packet;
use retinue::announce::RAND_HASH_LEN;
use retinue::destination::DestinationName;
use retinue::identity::PrivateIdentity;
use retinue::node::{
    Action, Actions, InterfaceId, InterruptionPermission, LINK_IDLE_TIMEOUT, Node, PauseBlocked,
};

mod budget;
mod forced;
mod pause;

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
    let request = sent(
        &a.open_link(b.destination(), IFACE, &[0x31; 64], seen)
            .unwrap(),
    );
    let proof = sent(&b.ingest(IFACE, &request, seen));
    let id = link_up(&a.ingest(IFACE, &proof, seen));
    (a, b, id)
}
