//! Node tests by topic; shared fixtures and helpers live here.

use alloc::vec;

use super::tables::derived_iv;
use super::*;
use crate::address_book::Ingested;
use crate::announce::{Announce, RAND_HASH_LEN, RATCHET_LEN};
use crate::announce_freshness::{
    AnnounceFreshnessCandidate, AnnounceFreshnessDecision, AnnounceFreshnessReject,
};
use crate::destination::DestinationName;
use crate::link::{self, LinkMode, LinkTrailer};
use crate::link_liveness;
use crate::packet::{HeaderType, PacketType};

mod announces;
mod bridges;
mod freshness;
mod limits;
mod links;
mod liveness;
mod recovery;
mod resources;
mod routes;
mod transit;

const IFACE: InterfaceId = 0;

fn fixture(name: &str) -> Packet {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    let raw = std::fs::read(std::format!("{path}{name}")).unwrap();
    Packet::decode(&raw).unwrap()
}

/// The first packet a set of actions wants sent.
fn sent<const N: usize>(actions: &Actions<N>) -> Option<Packet> {
    actions.iter().find_map(|a| match a {
        Action::Send { packet, .. } => Some(packet.clone()),
        _ => None,
    })
}

fn blob(bytes: [u8; RAND_HASH_LEN]) -> AnnounceBlob {
    AnnounceBlob::from_wire(bytes)
}

/// The link id a set of actions reports coming up.
fn link_up<const N: usize>(actions: &Actions<N>) -> Option<AddressHash> {
    actions.iter().find_map(|a| match a {
        Action::LinkUp { link_id } => Some(*link_id),
        _ => None,
    })
}

/// Two nodes that have not met.
fn pair() -> (Node<32, 8, 4>, Node<32, 8, 4>) {
    (
        Node::new(
            PrivateIdentity::from_secret_bytes(&[0x11; 64]),
            DestinationName::new("retinue", ["a"]).name_hash(),
        ),
        Node::new(
            PrivateIdentity::from_secret_bytes(&[0x22; 64]),
            DestinationName::new("retinue", ["b"]).name_hash(),
        ),
    )
}

/// Two nodes with a link already established between them.
fn linked() -> (Node<32, 8, 4>, Node<32, 8, 4>, AddressHash) {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
    let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
    let up = a.ingest(IFACE, &proof, 0);
    let id = link_up(&up).expect("link did not come up");
    let rtt = sent(&up).expect("the initiator reports its RTT");
    b.ingest(IFACE, &rtt, 0);
    (a, b, id)
}

fn node() -> Node {
    let identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let name = DestinationName::new("retinue", ["node"]);
    Node::new(identity, name.name_hash())
}
