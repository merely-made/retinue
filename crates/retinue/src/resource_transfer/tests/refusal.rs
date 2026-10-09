#[cfg(feature = "compression")]
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::Error;
use crate::link::{
    CTX_RESOURCE, CTX_RESOURCE_ADV, CTX_RESOURCE_ICL, CTX_RESOURCE_RCL, CTX_RESOURCE_REQ,
};
use crate::resource::{Outgoing, content, parse_request};

/// A cancel counts only if it decrypts on the link and names the resource in progress,
/// as RNS matches one. An unsealed cancel or one naming another resource is ignored.
#[test]
fn cancels_are_sealed_and_matched_by_resource_hash() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let mut sender =
        ResourceSender::publish(send_link.clone(), &payload(3000), [1, 2, 3, 4], &ivg());
    let mut receiver = ResourceReceiver::new(recv_link.clone());
    let hash = sender.resource_hash();
    assert!(
        !receiver
            .on_packet(&sender.advertisement(&ivg()), 0, &mut ivg)
            .is_empty()
    );

    // The receiver cancels the sender.
    let framed = recv_link.framed_packet(CTX_RESOURCE_RCL, hash.to_vec());
    sender.on_packet(&framed, 0, &mut ivg);
    assert!(
        !sender.is_canceled(),
        "an unsealed cancel is not the receiver's"
    );
    let other = recv_link.sealed_packet(CTX_RESOURCE_RCL, &[0x11; 32], &ivg());
    sender.on_packet(&other, 0, &mut ivg);
    assert!(!sender.is_canceled(), "a cancel for another resource");
    let real = recv_link.sealed_packet(CTX_RESOURCE_RCL, &hash, &ivg());
    sender.on_packet(&real, 0, &mut ivg);
    assert!(sender.is_canceled());

    // The initiator cancels the receiver.
    let framed = send_link.framed_packet(CTX_RESOURCE_ICL, hash.to_vec());
    receiver.on_packet(&framed, 0, &mut ivg);
    assert!(
        !receiver.is_canceled(),
        "an unsealed cancel is not the sender's"
    );
    let other = send_link.sealed_packet(CTX_RESOURCE_ICL, &[0x22; 32], &ivg());
    receiver.on_packet(&other, 0, &mut ivg);
    assert!(!receiver.is_canceled(), "a cancel for another resource");
    let real = send_link.sealed_packet(CTX_RESOURCE_ICL, &hash, &ivg());
    receiver.on_packet(&real, 0, &mut ivg);
    assert!(receiver.is_canceled());
    assert!(receiver.poll(u64::MAX, &mut ivg).is_empty());
}

/// Cancelling locally sends the sealed cancel RNS reads: ICL from the publisher, RCL
/// from the receiver, each naming the resource, and the far side stops on it.
#[test]
fn a_local_cancel_tells_the_peer() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let mut sender =
        ResourceSender::publish(send_link.clone(), &payload(3000), [1, 2, 3, 4], &ivg());
    let mut receiver = ResourceReceiver::new(recv_link.clone());
    assert!(receiver.cancel(&ivg()).is_none(), "nothing to cancel yet");
    receiver.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);

    let icl = sender.cancel(&ivg()).expect("a running publish cancels");
    assert_eq!(icl.context, CTX_RESOURCE_ICL);
    assert_eq!(recv_link.decrypt(&icl).unwrap(), sender.resource_hash());
    assert!(sender.cancel(&ivg()).is_none(), "once");
    receiver.on_packet(&icl, 0, &mut ivg);
    assert!(receiver.is_canceled());

    let mut receiver = ResourceReceiver::new(recv_link);
    let mut sender = ResourceSender::publish(send_link.clone(), &payload(3000), [5; 4], &ivg());
    receiver.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);
    let rcl = receiver.cancel(&ivg()).expect("a running receive cancels");
    assert_eq!(rcl.context, CTX_RESOURCE_RCL);
    assert_eq!(send_link.decrypt(&rcl).unwrap(), sender.resource_hash());
    assert!(receiver.poll(u64::MAX, &mut ivg).is_empty());
    sender.on_packet(&rcl, 0, &mut ivg);
    assert!(sender.is_canceled());
}

/// An accept hook sees the advertisement before any part is requested. An offer it
/// refuses is rejected on the wire, and the publisher stops on the rejection.
#[test]
fn an_offer_the_accept_hook_refuses_is_rejected() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = payload(5000);
    let mut sender = ResourceSender::publish(send_link.clone(), &data, [7; 4], &ivg());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let mut receiver = ResourceReceiver::new(recv_link).with_accept({
        let seen = seen.clone();
        move |advertisement| {
            *seen.lock().unwrap() = Some(advertisement.data_size);
            false
        }
    });
    let replies = receiver.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);
    assert_eq!(
        *seen.lock().unwrap(),
        Some(data.len() as u64),
        "the hook saw the size"
    );
    assert_eq!(replies.len(), 1, "a rejection, no part request");
    assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
    assert_eq!(receiver.failure(), Some(Error::ResourceRejected));
    deliver(replies, |packet| sender.on_packet(packet, 0, &mut ivg));
    assert!(sender.is_canceled());
}

/// A receiver bounded by a data size, as a request bounds its response, rejects a
/// larger offer and accepts one that fits.
#[test]
fn an_offer_past_the_size_limit_is_rejected() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = payload(5000);
    let sender = ResourceSender::publish(send_link, &data, [7; 4], &ivg());
    let mut small = ResourceReceiver::new(recv_link.clone()).with_max_data_size(4999);
    let replies = small.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
    assert_eq!(small.failure(), Some(Error::CapacityExceeded));

    let mut fits = ResourceReceiver::new(recv_link).with_max_data_size(5000);
    let replies = fits.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);
    assert_eq!(replies[0].context, CTX_RESOURCE_REQ);
    assert_eq!(fits.failure(), None);
}

/// An offer past the part ceiling is rejected on the wire rather than ignored, so the
/// publisher stops instead of re-advertising until it times out.
#[test]
fn an_offer_past_the_part_ceiling_is_rejected() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = sha256_counter_payload(5000);
    let mut sender = ResourceSender::publish(send_link, &data, [7; 4], &ivg());
    let mut receiver = ResourceReceiver::with_limits(recv_link, 4, 2);
    let replies = receiver.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
    assert_eq!(receiver.failure(), Some(Error::CapacityExceeded));
    deliver(replies, |packet| sender.on_packet(packet, 0, &mut ivg));
    assert!(sender.is_canceled());
}

/// A body that does not open under the link key fails the transfer with a cancel,
/// rather than leaving the publisher waiting for a proof that never comes.
#[test]
fn a_corrupt_body_fails_with_a_cancel() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let data = payload(2000);
    let random_hash = [9, 9, 9, 9];
    // Not a token this link sealed: every part matches its map hash, and the whole
    // does not open here.
    let token = sha256_counter_payload(2048);
    let out = Outgoing::new(&data, &token, random_hash, false);
    let advertisement =
        send_link.sealed_packet(CTX_RESOURCE_ADV, &out.advertisement().pack(), &ivg());
    let mut receiver = ResourceReceiver::new(recv_link);
    let mut replies = receiver.on_packet(&advertisement, 0, &mut ivg);
    while replies[0].context == CTX_RESOURCE_REQ {
        let request = parse_request(&send_link.decrypt(&replies[0]).unwrap()).unwrap();
        replies = out
            .serve(&request)
            .into_iter()
            .flat_map(|part| {
                receiver.on_packet(&send_link.framed_packet(CTX_RESOURCE, part), 0, &mut ivg)
            })
            .collect();
    }
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
    assert_eq!(send_link.decrypt(&replies[0]).unwrap(), out.resource_hash());
    assert_eq!(receiver.failure(), Some(Error::ResourceCorrupt));
    assert_eq!(receiver.data(), None);
}

/// A multi-segment advertisement (`l > 1`) is refused with a sealed receiver cancel
/// naming the resource, and the receiver never yields the first segment as if it were
/// the whole resource, even when every part of that segment arrives.
#[test]
fn a_multi_segment_advertisement_is_refused_and_never_yields_data() {
    let (send_link, recv_link) = link_pair();
    let mut ivg = iv_gen();
    let segment = payload(2000);
    let random_hash = [0x51, 0x52, 0x53, 0x54];
    let token = send_link.seal(&content(&segment, &random_hash), &ivg());
    let out = Outgoing::new(&segment, &token, random_hash, false).with_segment(
        1,
        2,
        4000,
        crate::resource::resource_hash(&segment, &random_hash),
    );
    let advertised = out.advertisement();
    assert_eq!(advertised.l, 2);
    let advertisement = send_link.sealed_packet(CTX_RESOURCE_ADV, &advertised.pack(), &ivg());

    let mut receiver = ResourceReceiver::new(recv_link.clone());
    let replies = receiver.on_packet(&advertisement, 0, &mut ivg);
    assert_eq!(replies.len(), 1, "one refusal, no part request");
    assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
    assert_eq!(
        send_link.decrypt(&replies[0]).unwrap(),
        out.resource_hash().to_vec(),
        "the cancel is sealed and names the resource, as RNS reads it"
    );
    assert_eq!(receiver.failure(), Some(Error::MultiSegmentResource));

    // A re-sent advertisement is refused again; every part of the segment is ignored.
    let again = receiver.on_packet(&advertisement, 0, &mut ivg);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].context, CTX_RESOURCE_RCL);
    let wanted = advertised.hashmap.as_chunks::<4>().0.to_vec();
    assert_eq!(
        wanted.len(),
        out.total_parts(),
        "the advert names every part"
    );
    let all = crate::resource::build_request(&out.resource_hash(), &wanted);
    let request = crate::resource::parse_request(&all).unwrap();
    for part in out.serve(&request) {
        let packet = send_link.framed_packet(CTX_RESOURCE, part);
        assert!(receiver.on_packet(&packet, 0, &mut ivg).is_empty());
    }
    assert!(receiver.poll(u64::MAX, &mut ivg).is_empty());
    assert!(!receiver.is_complete());
    assert_eq!(receiver.data(), None);
}

/// A compressed body that inflates past the receiver's limit fails the transfer with a
/// typed error and a sealed cancel, rather than being inflated and returned.
#[cfg(feature = "compression")]
#[test]
fn a_body_past_the_decompression_limit_fails_the_transfer() {
    let (send_link, recv_link) = link_pair();
    let data = vec![0_u8; 256 * 1024];
    let mut ivg = iv_gen();
    let mut sender = ResourceSender::publish(send_link.clone(), &data, [3, 1, 4, 1], &ivg());
    let mut receiver = ResourceReceiver::new(recv_link).with_max_decompressed_size(64 * 1024);
    let mut to_receiver = vec![sender.advertisement(&ivg())];
    let mut cancel = None;
    for _ in 0..100 {
        let mut to_sender = Vec::new();
        for packet in core::mem::take(&mut to_receiver) {
            to_sender.extend(receiver.on_packet(&packet, 0, &mut ivg));
        }
        for packet in to_sender {
            assert_ne!(packet.context, CTX_RESOURCE_PRF, "nothing is proved");
            if packet.context == CTX_RESOURCE_RCL {
                cancel = Some(packet.clone());
            }
            to_receiver.extend(sender.on_packet(&packet, 0, &mut ivg));
        }
        if to_receiver.is_empty() {
            break;
        }
    }
    assert_eq!(receiver.failure(), Some(Error::DecompressionLimit));
    assert_eq!(receiver.data(), None);
    assert!(receiver.poll(u64::MAX, &mut ivg).is_empty());
    let cancel = cancel.expect("the sender is told to stop");
    assert_eq!(
        send_link.decrypt(&cancel).unwrap(),
        sender.out.resource_hash().to_vec()
    );
    assert!(sender.is_canceled());
}
