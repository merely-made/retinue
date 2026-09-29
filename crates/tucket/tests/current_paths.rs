//! Software regressions against MeshCore 1.17.1's recorded wire rules.
//! These project-selected inputs are not stock-radio captures.

use tucket::identity::{Identity, LocalIdentity};
use tucket::mesh::{Forward, route_recv};
use tucket::node::{DirectRoute, Event, Node};
use tucket::packet::{Packet, ROUTE_DIRECT, ROUTE_FLOOD, ROUTE_TRANSPORT_FLOOD, payload_type};

#[test]
fn every_supported_width_appends_and_consumes_a_complete_hop() {
    let mut key = [0; 32];
    key[..3].copy_from_slice(&[0x11, 0x22, 0x33]);
    let identity = Identity::new(key);
    for width in 1..=3u8 {
        let mut packet = Packet::new(ROUTE_FLOOD, payload_type::TXT_MSG);
        packet.path_len = ((width - 1) << 6) | 1;
        packet.path = vec![0xaa; width as usize];
        packet.payload = vec![1, 2, 3];
        let Forward::Retransmit(forwarded) = route_recv(&packet, &identity, true, false) else {
            panic!("width {width} must forward");
        };
        let mut expected = packet.path.clone();
        expected.extend_from_slice(&key[..width as usize]);
        assert_eq!(forwarded.path, expected);
        assert_eq!(forwarded.path_hop_count(), 2);
        assert_eq!(
            Packet::decode(&forwarded.encode()).as_ref(),
            Some(forwarded.as_ref())
        );

        packet.header = (packet.header & !3) | ROUTE_DIRECT;
        packet.path = key[..width as usize].to_vec();
        packet.path.extend_from_slice(&vec![0xbb; width as usize]);
        packet.path_len = ((width - 1) << 6) | 2;
        let Forward::Retransmit(forwarded) = route_recv(&packet, &identity, true, false) else {
            panic!("width {width} must consume its full hop");
        };
        assert_eq!(forwarded.path, vec![0xbb; width as usize]);
        assert_eq!(forwarded.path_hop_count(), 1);
        assert_eq!(forwarded.payload, packet.payload);
        if width > 1 {
            packet.path[width as usize - 1] ^= 1;
            assert_eq!(route_recv(&packet, &identity, true, false), Forward::Drop);
        }
    }
}

#[test]
fn malformed_public_packet_fields_do_not_panic_or_create_a_frame() {
    let identity = Identity::new([0x11; 32]);
    for path_len in 0..=255u8 {
        let mut packet = Packet::new(ROUTE_DIRECT, payload_type::ACK);
        packet.path_len = path_len;
        packet.payload = vec![1, 2, 3, 4];
        // A public Packet can be constructed without going through the codec.
        assert_eq!(route_recv(&packet, &identity, true, false), Forward::Drop);
    }
}

#[test]
fn wider_flood_paths_stop_at_the_last_encodable_hop() {
    let identity = Identity::new([0x11; 32]);
    for width in 1..=3u8 {
        let max_count = 63.min(64 / width);
        let mut packet = Packet::new(ROUTE_FLOOD, payload_type::ACK);
        packet.path_len = ((width - 1) << 6) | max_count;
        packet.path = vec![0x22; max_count as usize * width as usize];
        packet.payload = vec![1, 2, 3, 4];
        assert_eq!(route_recv(&packet, &identity, true, false), Forward::Drop);
        packet.path.truncate(packet.path.len() - width as usize);
        packet.path_len -= 1;
        let Forward::Retransmit(forwarded) = route_recv(&packet, &identity, true, false) else {
            panic!("last complete hop fits");
        };
        assert!(Packet::decode(&forwarded.encode()).is_some());
    }
}

#[test]
fn trace_hash_matches_an_external_sha256_calculation() {
    let mut packet = Packet::new(ROUTE_FLOOD, payload_type::TRACE);
    packet.path_len = 0x41;
    packet.path = vec![0x11, 0x22];
    packet.payload = vec![1, 2, 3, 4];
    // SHA256(09 41 01 02 03 04)[..8], independently calculated with .NET.
    // MeshCore d929643 src/Packet.cpp hashes its one-byte path_len, not u16.
    assert_eq!(
        packet.packet_hash(),
        [0x3d, 0x18, 0x2a, 0xcf, 0x45, 0xbf, 0x58, 0x9c]
    );
    assert_ne!(
        packet.packet_hash(),
        [0xcc, 0x39, 0x71, 0x2d, 0xcf, 0x04, 0x26, 0xc9]
    );
}

#[test]
fn unsupported_version_and_scope_do_not_poison_later_plain_advert() {
    let mut sender = Node::new(LocalIdentity::from_seed([0x11; 32]), false);
    let plain = sender.advert_frame(100, b"peer");
    for kind in 0..2 {
        let mut receiver = Node::new(LocalIdentity::from_seed([0x22; 32]), true);
        let mut packet = Packet::decode(&plain).unwrap();
        if kind == 0 {
            packet.header |= 0x40;
        } else {
            packet.header = (packet.header & !3) | ROUTE_TRANSPORT_FLOOD;
            packet.transport_codes = [0x1234, 0];
        }
        let (events, out) = receiver.on_frame(&packet.encode());
        assert!(events.is_empty() && out.is_empty());
        assert!(receiver.contact(sender.my_hash()).is_none());
        assert!(matches!(
            receiver.on_frame(&plain).0.as_slice(),
            [Event::Advert { .. }]
        ));
    }
}

#[test]
fn two_repeaters_deliver_wider_direct_text_without_intermediate_app_delivery() {
    for width in 1..=3u8 {
        let mut alice = Node::new(LocalIdentity::from_seed([0x11; 32]), false);
        let mut bob = Node::new(LocalIdentity::from_seed([0x22; 32]), false);
        let mut first = Node::new(LocalIdentity::from_seed([0x33; 32]), true);
        let mut second = Node::new(LocalIdentity::from_seed([0x44; 32]), true);
        assert!(alice.set_flood_hash_size(width));
        assert!(!alice.set_flood_hash_size(4));
        assert_eq!(alice.flood_hash_size(), width);
        let advert = alice.advert_frame(1, b"alice");
        assert_eq!(Packet::decode(&advert).unwrap().path_hop_size(), width);
        alice.on_frame(&bob.advert_frame(1, b"bob"));
        bob.on_frame(&advert);
        let mut path = first.identity().pub_key[..width as usize].to_vec();
        path.extend_from_slice(&second.identity().pub_key[..width as usize]);
        assert!(alice.set_route(
            bob.my_hash(),
            DirectRoute::new(((width - 1) << 6) | 2, &path).unwrap()
        ));
        let (wire, expected_ack) = alice.text_frame(bob.my_hash(), 3, "two repeaters").unwrap();
        assert!(
            second.on_frame(&wire).1.is_empty(),
            "wrong hop must not poison dedup"
        );
        let (events, frames) = first.on_frame(&wire);
        assert!(events.is_empty());
        assert_eq!(frames.len(), 1);
        let (events, frames) = second.on_frame(&frames[0]);
        assert!(events.is_empty());
        assert_eq!(frames.len(), 1);
        assert!(
            matches!(bob.on_frame(&frames[0]).0.as_slice(), [Event::Message { message, ack, .. }]
            if message.text == "two repeaters" && *ack == expected_ack)
        );
    }
}
