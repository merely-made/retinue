//! The adaptive window, the watchdog and request batching, on a virtual clock.

use alloc::vec::Vec;

use super::pipe::{Pipe, pair, run};
use super::*;
use crate::Error;
use crate::link::{CTX_RESOURCE, CTX_RESOURCE_ICL, CTX_RESOURCE_RCL, CTX_RESOURCE_REQ};
use crate::resource::{HASHMAP_MAX_PARTS, WINDOW_MAX, parse_request};
use crate::resource_transfer::window::{WINDOW_INITIAL, WINDOW_MAX_SLOW, WINDOW_MAX_VERY_SLOW};

/// On a fast link the window opens a part per round to RNS's fast ceiling of 75, and a
/// request that reaches the end of the known hashmap asks for the next with its parts.
#[test]
fn a_fast_link_opens_the_window_to_the_fast_ceiling() {
    let data = sha256_counter_payload(2_000 * 464 - 100);
    let (mut sender, mut receiver) = pair(&data, 10);
    let stats = run(
        &mut sender,
        &mut receiver,
        Pipe::new(1_000_000, 5),
        Pipe::new(1_000_000, 5),
        600_000,
    );
    assert!(sender.is_done(), "{stats:?}");
    assert_eq!(receiver.data(), Some(data.as_slice()));
    assert_eq!(receiver.window_max(), WINDOW_MAX);
    assert_eq!(stats.max_window, WINDOW_MAX);
    assert_eq!(stats.empty_requests, 0, "no request only solicits hashmap");
    assert_eq!(stats.parts_heard, 2_000, "no part is sent twice");
}

/// About 2,000 parts at a carried full window: one request per 74-part hashmap segment,
/// where a separate hashmap solicitation per segment took two.
#[test]
fn two_thousand_parts_take_one_round_per_hashmap_segment() {
    let data = sha256_counter_payload(2_000 * 464 - 100);
    let (mut sender, receiver) = pair(&data, 10);
    let mut link = receiver.link.clone();
    link.set_resource_carry(WindowCarry {
        window: WINDOW_MAX,
        rate: Some(1_000_000),
    });
    let mut receiver = ResourceReceiver::new(link).with_timing(Timing { rtt: 10, floor: 0 });
    let stats = run(
        &mut sender,
        &mut receiver,
        Pipe::new(1_000_000, 5),
        Pipe::new(1_000_000, 5),
        600_000,
    );
    assert!(sender.is_done(), "{stats:?}");
    assert_eq!(stats.empty_requests, 0);
    assert_eq!(
        stats.requests,
        2_000_usize.div_ceil(HASHMAP_MAX_PARTS),
        "{stats:?}"
    );
}

/// A link between 2 and 50 kbit/s keeps RNS's slow ceiling of 10.
#[test]
fn a_slow_link_keeps_the_slow_ceiling() {
    let data = sha256_counter_payload(60 * 464);
    let (mut sender, mut receiver) = pair(&data, 1_000);
    let stats = run(
        &mut sender,
        &mut receiver,
        Pipe::new(1_000, 50),
        Pipe::new(1_000, 50),
        3_600_000,
    );
    assert!(sender.is_done(), "{stats:?}");
    assert_eq!(receiver.window_max(), WINDOW_MAX_SLOW);
    assert_eq!(stats.max_window, WINDOW_MAX_SLOW);
    assert_eq!(stats.parts_heard, sender.out.total_parts());
}

/// Two rounds under 2 kbit/s cap the ceiling at 4; the window stops growing there.
#[test]
fn a_very_slow_link_caps_the_ceiling() {
    let data = sha256_counter_payload(24 * 464);
    let (mut sender, mut receiver) = pair(&data, 3_000);
    let stats = run(
        &mut sender,
        &mut receiver,
        Pipe::new(150, 200),
        Pipe::new(150, 200),
        3_600_000,
    );
    assert!(sender.is_done(), "{stats:?}");
    assert_eq!(receiver.window_max(), WINDOW_MAX_VERY_SLOW);
    // RNS caps the ceiling after the round that grew the window, which then stays put.
    assert_eq!(stats.max_window, WINDOW_INITIAL + 2);
}

/// Over a lossy link the transfer completes, the window narrows on timeouts, and parts
/// sent twice stay few.
#[test]
fn a_lossy_link_completes_with_few_duplicate_parts() {
    let data = sha256_counter_payload(400 * 464);
    let (mut sender, mut receiver) = pair(&data, 40);
    let stats = run(
        &mut sender,
        &mut receiver,
        Pipe::new(20_000, 20).lossy(7, 100),
        Pipe::new(20_000, 20).lossy(0x5151, 100),
        3_600_000,
    );
    assert!(sender.is_done(), "{stats:?}");
    assert_eq!(receiver.data(), Some(data.as_slice()));
    let duplicates = stats.parts_heard - sender.out.total_parts();
    assert!(duplicates <= 8, "{stats:?}");
}

/// The window and expected rate carry to the next transfer on the link.
#[test]
fn the_window_carries_to_the_next_transfer() {
    let data = sha256_counter_payload(200 * 464);
    let (mut sender, mut receiver) = pair(&data, 10);
    run(
        &mut sender,
        &mut receiver,
        Pipe::new(1_000_000, 5),
        Pipe::new(1_000_000, 5),
        60_000,
    );
    let carry = receiver.carry();
    assert!(carry.window > WINDOW_MAX_SLOW, "{carry:?}");
    let (_, mut recv_link) = link_pair();
    recv_link.set_resource_carry(carry);
    assert_eq!(ResourceReceiver::new(recv_link).window(), carry.window);
}

/// A configured window is a ceiling the adaptive one never passes.
#[test]
fn a_configured_window_is_a_ceiling() {
    let data = sha256_counter_payload(100 * 464);
    let (mut sender, _) = pair(&data, 10);
    let (_, recv_link) = link_pair();
    let mut receiver = ResourceReceiver::with_request_window(recv_link, 3);
    let stats = run(
        &mut sender,
        &mut receiver,
        Pipe::new(1_000_000, 5),
        Pipe::new(1_000_000, 5),
        60_000,
    );
    assert!(sender.is_done());
    assert_eq!(stats.max_window, 3);
}

/// More parts than one advertisement names: the request that reaches the end of the
/// known hashes asks for those parts and the next hashmap together.
#[test]
fn the_exhausted_request_carries_the_known_parts() {
    let data = sha256_counter_payload(100 * 464);
    let (send_link, mut recv_link) = link_pair();
    let sender = ResourceSender::publish(send_link, &data, [2; 4], &[3; IV_LEN]);
    recv_link.set_resource_carry(WindowCarry {
        window: WINDOW_MAX,
        rate: None,
    });
    let mut receiver = ResourceReceiver::new(recv_link);
    let mut ivg = iv_gen();
    let request = receiver.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);
    let request = parse_request(&sender.link.decrypt(&request[0]).unwrap()).unwrap();
    assert!(request.exhausted);
    assert_eq!(request.wanted.len(), HASHMAP_MAX_PARTS);
    // A re-sent advertisement of the transfer under way is ignored (`Resource.py` 224-240).
    let again = sender.advertisement(&ivg());
    assert!(receiver.on_packet(&again, 1, &mut ivg).is_empty());
}

/// The receiver re-requests on each overdue part timeout, backing off and narrowing the
/// window, and after `MAX_RETRIES` of them gives up with a cancel.
#[test]
fn a_silent_sender_times_the_receiver_out() {
    let data = payload(20 * 464);
    let (sender, mut receiver) = pair(&data, 100);
    let mut ivg = iv_gen();
    let first = receiver.on_packet(&sender.advertisement(&ivg()), 0, &mut ivg);
    assert_eq!(first[0].context, CTX_RESOURCE_REQ);
    let mut waits = Vec::new();
    let mut last = 0;
    for _ in 0..MAX_RETRIES {
        let deadline = receiver.deadline().unwrap();
        assert!(
            receiver.poll(deadline - 1, &mut ivg).is_empty(),
            "not before"
        );
        let retry = receiver.poll(deadline, &mut ivg);
        assert_eq!(retry[0].context, CTX_RESOURCE_REQ);
        waits.push(deadline - last);
        last = deadline;
    }
    assert!(waits.windows(2).all(|w| w[1] >= w[0]), "{waits:?}");
    assert_eq!(receiver.window(), 2, "narrowed to the floor");
    let cancel = receiver.poll(receiver.deadline().unwrap(), &mut ivg);
    assert_eq!(cancel[0].context, CTX_RESOURCE_RCL);
    assert_eq!(receiver.failure(), Some(Error::ResourceTimedOut));
    assert_eq!(receiver.deadline(), None);
}

/// An unanswered advertisement is re-sent `MAX_ADV_RETRIES` times, then the sender
/// cancels; a sender whose receiver goes quiet mid-transfer cancels too.
#[test]
fn a_silent_receiver_times_the_sender_out() {
    let data = sha256_counter_payload(20 * 464);
    let (mut sender, _) = pair(&data, 100);
    let mut ivg = iv_gen();
    sender.advertise(0, &ivg());
    for _ in 0..MAX_ADV_RETRIES {
        let deadline = sender.deadline().unwrap();
        assert!(sender.poll(deadline - 1, &mut ivg).is_none());
        let again = sender.poll(deadline, &mut ivg).unwrap();
        assert_eq!(again.context, crate::link::CTX_RESOURCE_ADV);
    }
    let cancel = sender.poll(sender.deadline().unwrap(), &mut ivg).unwrap();
    assert_eq!(cancel.context, CTX_RESOURCE_ICL);
    assert!(sender.timed_out() && sender.is_canceled());

    let (mut sender, mut receiver) = pair(&data, 100);
    let adv = sender.advertise(0, &ivg());
    let request = receiver.on_packet(&adv, 50, &mut ivg);
    let parts = sender.on_packet(&request[0], 100, &mut ivg);
    assert_eq!(parts[0].context, CTX_RESOURCE);
    let deadline = sender.deadline().unwrap();
    assert!(
        deadline > 60_000,
        "RNS waits out every receiver retry: {deadline}"
    );
    assert_eq!(
        sender.poll(deadline, &mut ivg).unwrap().context,
        CTX_RESOURCE_ICL
    );
    assert!(sender.timed_out());
}
