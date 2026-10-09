//! Multi-segment resources through `SegmentedSender` and `SegmentedReceiver`.

use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::Error;
use crate::link::{CTX_RESOURCE_ADV, CTX_RESOURCE_RCL};
use crate::resource::{
    Advertisement, FLAG_REQUEST, FLAG_RESPONSE, FLAG_SPLIT, MAX_SEGMENT_SIZE, Outgoing, content,
};

/// msgpack `{"n": 7}`.
const METADATA: &[u8] = b"\x81\xa1n\x07";

fn receiver_for(link: &Link) -> SegmentedReceiver {
    let make = link.clone();
    SegmentedReceiver::new(link.clone(), move || ResourceReceiver::new(make.clone()))
}

fn advertised(link: &Link, packet: &Packet) -> Advertisement {
    Advertisement::parse(&link.decrypt(packet).unwrap()).unwrap()
}

/// Exchange packets until `stop` holds, returning every advertisement the sender sent.
fn exchange<D: AsRef<[u8]>>(
    sender: &mut SegmentedSender<D>,
    receiver: &mut SegmentedReceiver,
    to_receiver: &mut Vec<Packet>,
    ivg: &mut impl FnMut() -> [u8; IV_LEN],
    stop: impl Fn(&SegmentedSender<D>, &SegmentedReceiver) -> bool,
) -> Vec<Packet> {
    let mut adverts = Vec::new();
    for _ in 0..100_000 {
        if stop(sender, receiver) {
            return adverts;
        }
        let mut to_sender = Vec::new();
        for packet in core::mem::take(to_receiver) {
            if packet.context == CTX_RESOURCE_ADV {
                adverts.push(packet.clone());
            }
            to_sender.extend(receiver.on_packet(&packet, &mut *ivg));
        }
        for packet in to_sender {
            to_receiver.extend(sender.on_packet(&packet, &mut *ivg));
        }
    }
    panic!("the exchange stalled");
}

#[test]
fn segment_count_follows_rns() {
    assert_eq!(segment_count(0), 1);
    assert_eq!(segment_count(MAX_SEGMENT_SIZE), 1);
    assert_eq!(segment_count(MAX_SEGMENT_SIZE + 1), 2);
    assert_eq!(segment_count(5 * 1024 * 1024 / 2), 3);
}

/// Three segments with metadata: each advertised only after the previous is proved, all
/// naming the first segment's hash, and the receiver completing only at the last.
#[test]
fn a_split_resource_with_metadata_arrives_whole() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = sha256_counter_payload(2 * MAX_SEGMENT_SIZE + 1000);
    let mut sender = SegmentedSender::new(
        send_link,
        data.as_slice(),
        Some(METADATA),
        ResourceKind::Data,
        [1, 2, 3, 4],
        &ivg(),
    )
    .unwrap();
    assert_eq!(sender.segment(), (1, 3));
    let mut receiver = receiver_for(&recv_link);
    let mut to_receiver = vec![sender.advertisement(&ivg())];
    let adverts = exchange(
        &mut sender,
        &mut receiver,
        &mut to_receiver,
        &mut ivg,
        |s, r| {
            assert!(
                !r.is_complete() || r.segments_proved() == 3,
                "complete only at the last"
            );
            s.is_done() && r.is_complete()
        },
    );

    let adverts: Vec<_> = adverts.iter().map(|p| advertised(&recv_link, p)).collect();
    assert_eq!(
        adverts.iter().map(|a| a.i).collect::<Vec<_>>(),
        [1, 2, 3],
        "one advertisement per segment, in order"
    );
    let total = (data.len() + 3 + METADATA.len()) as u64;
    for adv in &adverts {
        assert_eq!(adv.l, 3);
        assert_eq!(
            adv.data_size, total,
            "d is the whole resource, metadata framing included"
        );
        assert_eq!(adv.original_hash, sender.original_hash().to_vec());
        assert_ne!(adv.flags & FLAG_SPLIT, 0);
        assert!(
            adv.has_metadata(),
            "every segment carries the flag, as RNS sets it"
        );
    }
    assert_eq!(adverts[0].original_hash, adverts[0].resource_hash);
    assert_eq!(receiver.metadata(), Some(METADATA));
    assert_eq!(receiver.data(), Some(data.as_slice()));
    assert_eq!(receiver.kind(), Some(ResourceKind::Data));
    let (taken, metadata) = receiver.take_payload().unwrap();
    assert_eq!((taken, metadata.as_deref()), (data, Some(METADATA)));
    assert!(
        receiver.proof_packet().is_some(),
        "the last proof stays after the take"
    );
}

/// Between segments: a lost proof is answered when the proved segment is offered again,
/// and another resource's offer is refused without disturbing this one.
#[test]
fn between_segments_a_reoffer_gets_its_proof_and_a_stranger_is_refused() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = payload(MAX_SEGMENT_SIZE + 5000);
    let mut sender = SegmentedSender::new(
        send_link.clone(),
        data.as_slice(),
        None,
        ResourceKind::Data,
        [5; 4],
        &ivg(),
    )
    .unwrap();
    let mut receiver = receiver_for(&recv_link);
    let first = sender.advertisement(&ivg());
    let mut to_receiver = vec![first.clone()];
    exchange(
        &mut sender,
        &mut receiver,
        &mut to_receiver,
        &mut ivg,
        |s, _| s.segment().0 == 2,
    );
    assert_eq!(receiver.segments_proved(), 1);

    let proof = receiver.on_packet(&first, &mut ivg);
    assert_eq!(proof.len(), 1);
    assert_eq!(Some(&proof[0]), receiver.last_proof());

    let small = payload(800);
    let stranger = SegmentedSender::new(
        send_link.clone(),
        small,
        None,
        ResourceKind::Data,
        [6; 4],
        &ivg(),
    )
    .unwrap();
    let refusal = receiver.on_packet(&stranger.advertisement(&ivg()), &mut ivg);
    assert_eq!(refusal.len(), 1);
    assert_eq!(refusal[0].context, CTX_RESOURCE_RCL);
    assert_eq!(
        send_link.decrypt(&refusal[0]).unwrap(),
        stranger.resource_hash()
    );
    assert_eq!(receiver.failure(), None);

    exchange(
        &mut sender,
        &mut receiver,
        &mut to_receiver,
        &mut ivg,
        |s, r| s.is_done() && r.is_complete(),
    );
    assert_eq!(receiver.data(), Some(data.as_slice()));
}

/// A segment out of sequence fails the whole resource with a refusal.
#[test]
fn a_segment_out_of_sequence_fails_the_resource() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = payload(2 * MAX_SEGMENT_SIZE + 10);
    let mut sender = SegmentedSender::new(
        send_link.clone(),
        data.as_slice(),
        None,
        ResourceKind::Data,
        [7; 4],
        &ivg(),
    )
    .unwrap();
    let mut receiver = receiver_for(&recv_link);
    let mut to_receiver = vec![sender.advertisement(&ivg())];
    exchange(
        &mut sender,
        &mut receiver,
        &mut to_receiver,
        &mut ivg,
        |s, _| s.segment().0 == 2,
    );

    // Segment 3 offered where segment 2 is due.
    let tail = &data[2 * MAX_SEGMENT_SIZE..];
    let random_hash = [8; 4];
    let token = send_link.seal(&content(tail, &random_hash), &ivg());
    let skipped = Outgoing::new(tail, &token, random_hash, false).with_segment(
        3,
        3,
        data.len() as u64,
        sender.original_hash(),
    );
    let packet = send_link.sealed_packet(CTX_RESOURCE_ADV, &skipped.advertisement().pack(), &ivg());
    let refusal = receiver.on_packet(&packet, &mut ivg);
    assert_eq!(refusal.len(), 1);
    assert_eq!(refusal[0].context, CTX_RESOURCE_RCL);
    assert_eq!(receiver.failure(), Some(Error::ResourceCorrupt));
    assert_eq!(receiver.data(), None);
}

/// First offers that cannot begin a resource, or exceed the size cap, are refused.
#[test]
fn bad_first_offers_are_refused() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let segment = payload(2000);
    let random_hash = [9; 4];
    let token = send_link.seal(&content(&segment, &random_hash), &ivg());
    let offer = |index, total, size| {
        let out = Outgoing::new(&segment, &token, random_hash, false)
            .with_segment(index, total, size, [0x11; 32]);
        send_link.sealed_packet(CTX_RESOURCE_ADV, &out.advertisement().pack(), &[3; 16])
    };
    let big = 3 * MAX_SEGMENT_SIZE as u64;
    for (packet, max, error) in [
        (offer(2, 3, big), usize::MAX, Error::ResourceCorrupt),
        (offer(1, 2, big), usize::MAX, Error::ResourceCorrupt),
        (offer(1, 3, big), MAX_SEGMENT_SIZE, Error::CapacityExceeded),
    ] {
        let mut receiver = receiver_for(&recv_link).with_max_size(max);
        let replies = receiver.on_packet(&packet, &mut ivg);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
        assert_eq!(receiver.failure(), Some(error));
    }
    // A well-formed first segment is taken.
    let mut receiver = receiver_for(&recv_link);
    let replies = receiver.on_packet(&offer(1, 3, big), &mut ivg);
    assert!(replies.iter().all(|p| p.context != CTX_RESOURCE_RCL));
    assert_eq!(receiver.segment(), Some((1, 3)));
}

/// Request and response Resources carry their id in `q` with the `u` or `p` flag, and a
/// filter ignores the offers it refuses without a word.
#[test]
fn request_resources_are_flagged_and_filterable() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let id = [0x42; 16];
    let request = SegmentedSender::new(
        send_link.clone(),
        payload(3000),
        None,
        ResourceKind::Request(id),
        [1; 4],
        &ivg(),
    )
    .unwrap();
    let response = SegmentedSender::new(
        send_link.clone(),
        payload(3000),
        None,
        ResourceKind::Response(id),
        [2; 4],
        &ivg(),
    )
    .unwrap();
    let req_adv = advertised(&recv_link, &request.advertisement(&ivg()));
    let resp_adv = advertised(&recv_link, &response.advertisement(&ivg()));
    assert_eq!(req_adv.q.as_deref(), Some(&id[..]));
    assert_eq!(
        req_adv.flags & (FLAG_REQUEST | FLAG_RESPONSE | FLAG_SPLIT),
        FLAG_REQUEST
    );
    assert_eq!(
        resp_adv.flags & (FLAG_REQUEST | FLAG_RESPONSE | FLAG_SPLIT),
        FLAG_RESPONSE
    );

    let requests_only =
        || receiver_for(&recv_link).with_filter(|adv| adv.flags & FLAG_REQUEST != 0);
    let mut receiver = requests_only();
    assert!(
        receiver
            .on_packet(&response.advertisement(&ivg()), &mut ivg)
            .is_empty()
    );
    assert_eq!((receiver.kind(), receiver.failure()), (None, None));

    let mut sender = request;
    let mut receiver = requests_only();
    let mut to_receiver = vec![sender.advertisement(&ivg())];
    exchange(
        &mut sender,
        &mut receiver,
        &mut to_receiver,
        &mut ivg,
        |s, r| s.is_done() && r.is_complete(),
    );
    assert_eq!(receiver.kind(), Some(ResourceKind::Request(id)));
    assert_eq!(receiver.data(), Some(payload(3000).as_slice()));
}

/// Drive a whole-resource `sender` into `receiver` until neither has more to say.
fn drive_whole(
    sender: &mut ResourceSender,
    receiver: &mut SegmentedReceiver,
    ivg: &mut impl FnMut() -> [u8; IV_LEN],
) {
    let mut to_receiver = vec![sender.advertisement(&ivg())];
    while !to_receiver.is_empty() {
        let to_sender = deliver(core::mem::take(&mut to_receiver), |p| {
            receiver.on_packet(p, &mut *ivg)
        });
        to_receiver = deliver(to_sender, |p| sender.on_packet(p, &mut *ivg));
    }
}

/// A whole resource whose `d` understates its body is held to `d`: an uncompressed body
/// fails on arrival, and a compressed one stops inflating there, under a session cap far
/// below the default decompression bound.
#[test]
fn a_whole_resource_larger_than_advertised_fails() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let random_hash = [5; 4];
    let understated = |data: &[u8], body: &[u8], compressed| {
        let token = send_link.seal(&content(body, &random_hash), &[4; 16]);
        let out =
            Outgoing::new(data, &token, random_hash, compressed).with_segment(1, 1, 100, [0; 32]);
        ResourceSender::from_outgoing(send_link.clone(), out)
    };

    let data = payload(2000);
    let mut sender = understated(&data, &data, false);
    let mut receiver = receiver_for(&recv_link).with_max_size(10_000);
    drive_whole(&mut sender, &mut receiver, &mut ivg);
    assert_eq!(receiver.failure(), Some(Error::ResourceCorrupt));
    assert_eq!(receiver.data(), None);

    #[cfg(feature = "compression")]
    {
        let data = vec![0_u8; 200_000];
        let mut sender = understated(&data, &crate::resource::compress(&data), true);
        let mut receiver = receiver_for(&recv_link).with_max_size(10_000);
        drive_whole(&mut sender, &mut receiver, &mut ivg);
        assert_eq!(receiver.failure(), Some(Error::DecompressionLimit));
        assert_eq!(receiver.data(), None);
    }
}
