use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::link::CTX_RESOURCE_REQ;
use crate::resource::{Advertisement, Incoming, Outgoing};

/// A clean transfer with no loss: advertise, request, serve, prove — end to end.
#[test]
fn transfers_a_small_resource() {
    let (send_link, recv_link) = link_pair();
    let data = payload(3000);
    let mut ivg = iv_gen();
    let mut sender = ResourceSender::publish(send_link, &data, [0xAB, 0xCD, 0xEF, 0x01], &ivg());
    let mut receiver = ResourceReceiver::new(recv_link);

    let mut to_receiver = vec![sender.advertisement(&ivg())];
    let mut to_sender: Vec<Packet> = Vec::new();
    for _ in 0..100 {
        for pkt in core::mem::take(&mut to_receiver) {
            to_sender.extend(receiver.on_packet(&pkt, 0, &mut ivg));
        }
        for pkt in core::mem::take(&mut to_sender) {
            to_receiver.extend(sender.on_packet(&pkt, 0, &mut ivg));
        }
        if sender.is_done() && receiver.is_complete() {
            break;
        }
    }
    assert!(sender.is_done(), "sender saw the proof");
    assert_eq!(receiver.data(), Some(data.as_slice()), "payload recovered");
}

#[cfg(feature = "compression")]
#[test]
fn sender_compresses_when_the_encoded_body_is_smaller() {
    let (send_link, recv_link) = link_pair();
    let data = vec![b'a'; 128 * 1024];
    let mut ivg = iv_gen();
    let mut sender = ResourceSender::publish(send_link, &data, [0xAB, 0xCD, 0xEF, 0x01], &ivg());
    let advertisement = sender.advertisement(&ivg());
    let plain = recv_link.decrypt(&advertisement).unwrap();
    let advertised = Advertisement::parse(&plain).unwrap();
    assert_ne!(advertised.flags & crate::resource::FLAG_COMPRESSED, 0);
    assert!(advertised.transfer_size < data.len() as u64);

    let mut receiver = ResourceReceiver::new(recv_link);
    let mut to_receiver = vec![advertisement];
    let mut to_sender: Vec<Packet> = Vec::new();
    for _ in 0..100 {
        for packet in core::mem::take(&mut to_receiver) {
            to_sender.extend(receiver.on_packet(&packet, 0, &mut ivg));
        }
        for packet in core::mem::take(&mut to_sender) {
            to_receiver.extend(sender.on_packet(&packet, 0, &mut ivg));
        }
        if sender.is_done() && receiver.is_complete() {
            break;
        }
    }
    assert!(sender.is_done(), "sender saw the proof");
    assert_eq!(receiver.data(), Some(data.as_slice()), "payload recovered");
}

#[cfg(feature = "compression")]
#[test]
fn sender_keeps_an_incompressible_body_plain() {
    let (send_link, recv_link) = link_pair();
    let data = sha256_counter_payload(128 * 1024);
    let mut ivg = iv_gen();
    let sender = ResourceSender::publish(send_link, &data, [0xA1, 0xB2, 0xC3, 0xD4], &ivg());
    let advertisement = sender.advertisement(&ivg());
    let plain = recv_link.decrypt(&advertisement).unwrap();
    let advertised = Advertisement::parse(&plain).unwrap();
    assert_eq!(advertised.flags & crate::resource::FLAG_COMPRESSED, 0);
    assert!(advertised.transfer_size > data.len() as u64);
}

#[test]
fn negotiated_mtu_bounds_resource_frames() {
    let (send_link, recv_link) = link_pair_with_mtu(255);
    let data = payload(4_096);
    let mut ivg = iv_gen();
    let mut sender = ResourceSender::publish(send_link, &data, [0x10, 0x20, 0x30, 0x40], &ivg());
    let mut receiver = ResourceReceiver::with_request_window(recv_link, 1);
    let mut to_receiver = vec![sender.advertisement(&ivg())];
    let mut to_sender = Vec::new();

    for _ in 0..500 {
        for packet in core::mem::take(&mut to_receiver) {
            assert!(packet.encoded_len() <= 255);
            to_sender.extend(receiver.on_packet(&packet, 0, &mut ivg));
        }
        for packet in core::mem::take(&mut to_sender) {
            assert!(packet.encoded_len() <= 255);
            let replies = sender.on_packet(&packet, 0, &mut ivg);
            assert!(replies.len() <= 1, "one-part request window");
            to_receiver.extend(replies);
        }
        if sender.is_done() && receiver.is_complete() {
            break;
        }
    }

    assert!(sender.has_started());
    assert!(sender.is_done());
    assert_eq!(receiver.data(), Some(data.as_slice()));
}

/// A multi-part transfer over a lossy pipe, exercising retransmission of the
/// advertisement, requests, parts, and the proof, and the HMU path for a large hashmap.
#[test]
fn transfers_a_large_resource_over_loss() {
    // Big enough to need many parts and stream the hashmap over more than one HMU.
    let data = payload(45_000);
    let (mut sender, mut receiver) = super::pipe::pair(&data, 10);
    super::pipe::run(
        &mut sender,
        &mut receiver,
        super::pipe::Pipe::new(100_000, 3).lossy(7, 150),
        super::pipe::Pipe::new(100_000, 3).lossy(0x5151, 150),
        3_600_000,
    );
    assert!(sender.is_done(), "sender saw the proof over loss");
    assert_eq!(
        receiver.data(),
        Some(data.as_slice()),
        "large payload recovered exactly over loss"
    );
}

/// Metadata rides in front of the data, flagged in the advertisement, and comes out
/// separately; the proof covers both, as RNS's does.
#[test]
fn metadata_round_trips_beside_the_data() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = payload(9000);
    // msgpack {"name": "x.bin"}
    let metadata = b"\x81\xa4name\xa5x.bin";
    let mut sender =
        ResourceSender::publish_with_metadata(send_link, &data, metadata, [3, 3, 3, 3], &ivg())
            .unwrap();
    let advertisement = sender.advertisement(&ivg());
    let advertised = Advertisement::parse(&recv_link.decrypt(&advertisement).unwrap()).unwrap();
    assert!(advertised.has_metadata());
    assert_eq!(
        advertised.data_size,
        (3 + metadata.len() + data.len()) as u64
    );

    let mut receiver = ResourceReceiver::new(recv_link);
    let mut to_receiver = vec![advertisement];
    for _ in 0..100 {
        let to_sender = deliver(core::mem::take(&mut to_receiver), |packet| {
            receiver.on_packet(packet, 0, &mut ivg)
        });
        to_receiver = deliver(to_sender, |packet| sender.on_packet(packet, 0, &mut ivg));
        if sender.is_done() {
            break;
        }
    }
    assert!(sender.is_done(), "the proof over metadata and data matched");
    assert_eq!(receiver.data(), Some(data.as_slice()));
    assert_eq!(receiver.metadata(), Some(&metadata[..]));
}

/// Byte-identical parts share a map hash, and serving one serves every slot it fills, so
/// the sender still reaches `awaiting_proof`.
#[test]
fn serving_a_repeated_part_counts_every_slot_it_fills() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let part_size = 100;
    let mut token = vec![0_u8; 3 * part_size];
    token[part_size] = 1; // parts 0 and 2 are identical, part 1 differs
    let out = Outgoing::from_token(b"data", token, [1, 2, 3, 4], false, part_size);
    let mut sender = ResourceSender::from_outgoing(send_link, out);
    let plain = recv_link.decrypt(&sender.advertisement(&ivg())).unwrap();
    let incoming = Incoming::new(&Advertisement::parse(&plain).unwrap()).unwrap();
    let mut wanted = incoming.missing_known();
    assert_eq!(wanted.len(), 3);
    assert_eq!(wanted[0], wanted[2], "the identical parts share a hash");
    // A receiver asks once for the shared hash.
    wanted.truncate(2);
    let request = recv_link.sealed_packet(CTX_RESOURCE_REQ, &incoming.request(&wanted), &ivg());
    assert_eq!(sender.on_packet(&request, 0, &mut ivg).len(), 2);
    assert!(sender.awaiting_proof(), "all three slots were served");
}
