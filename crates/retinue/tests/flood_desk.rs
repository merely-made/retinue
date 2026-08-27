//! The address book's cap, exercised by a flood of valid announces.
//!
//! Forty genuine announces from forty distinct identities: the book learns its capacity
//! and refuses the rest, visibly. Also a monument to a bench lesson: the first flood
//! generator varied only byte 0 of the x25519 secret, which clamping (`k[0] &= 248`)
//! collapses into five distinct keys — so the "flood" was five peers refreshing, and the
//! hardware that reported peers=5 was right while the harness was wrong.

use retinue::announce::{self, AnnounceBlob, RAND_HASH_LEN};
use retinue::destination::DestinationName;
use retinue::identity::PrivateIdentity;
use retinue::node::Node;

fn flood_identity(index: u8) -> PrivateIdentity {
    let mut seed = [0x50_u8; 64];
    // Byte 1, not byte 0: x25519 clamping rewrites byte 0, and an index placed there
    // collapses forty identities into five.
    seed[1] = index;
    PrivateIdentity::from_secret_bytes(&seed)
}

#[test]
fn a_flood_fills_the_book_to_its_cap_and_refusals_are_counted() {
    let mut node = Node::<32, 8, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x99; 64]),
        DestinationName::new("retinue", ["node"]).name_hash(),
    );

    for index in 0..40u8 {
        let identity = flood_identity(index);
        let name = DestinationName::new("retinue", ["floodpeer"]);
        let mut rand_hash = [0_u8; RAND_HASH_LEN];
        rand_hash[..4].copy_from_slice(&u32::from(index).to_le_bytes());
        let blob = AnnounceBlob::from_wire(rand_hash);
        let packet = announce::build(&identity, name.name_hash(), &blob, None, &[]);
        let _ = node.ingest(0, &packet, 0);
    }

    assert_eq!(
        node.peers().len(),
        32,
        "the book holds exactly its capacity"
    );
    assert_eq!(node.refused_peers(), 8, "and every refusal is counted");
}
