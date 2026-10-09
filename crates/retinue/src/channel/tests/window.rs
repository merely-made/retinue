//! Send-window growth and shrink, RTT tiers, and retransmit timing.

use alloc::vec;
use alloc::vec::Vec;

use crate::channel::window::{
    FAST_RATE_THRESHOLD, WINDOW_MAX_MEDIUM, WINDOW_MAX_SLOW, WINDOW_MIN_LIMIT_MEDIUM,
};
use crate::channel::{
    Channel, ChannelError, DEFAULT_MAX_TRIES, DEFAULT_RETX_TIMEOUT, STREAM_MSGTYPE,
    WINDOW_FLEXIBILITY, WINDOW_INITIAL, WINDOW_MAX,
};

/// Count how many envelopes `channel` puts on the wire to deliver `messages` messages over
/// a lossless pipe whose one-way delay is `rtt/2` ticks (so a data->proof round trip is
/// `rtt` ticks). Each sequence's proof returns once, `rtt` ticks after its first send;
/// retransmits issued before then are the waste this measures.
fn transmissions_over_rtt(
    mut channel: Channel,
    rtt: u64,
    messages: usize,
) -> Result<usize, (usize, ChannelError)> {
    use alloc::collections::BTreeMap;
    use std::collections::HashSet;
    for m in 0..messages {
        channel
            .send(vec![m as u8])
            .expect("the send queue has room");
    }
    let mut proof_at: BTreeMap<u64, Vec<u16>> = BTreeMap::new();
    let mut scheduled: HashSet<u16> = HashSet::new();
    let mut total = 0usize;
    for now in 0..2_000_000u64 {
        if let Some(seqs) = proof_at.remove(&now) {
            for s in seqs {
                channel.on_proof(s, now);
            }
        }
        for env in channel.poll_transmit(now) {
            total += 1;
            if scheduled.insert(env.sequence) {
                proof_at.entry(now + rtt).or_default().push(env.sequence);
            }
        }
        if let Some(error) = channel.error() {
            return Err((total, error));
        }
        if channel.send_idle() {
            break;
        }
    }
    assert!(channel.send_idle(), "the transfer completed");
    Ok(total)
}

/// The adaptive retransmit timeout must not storm a slow medium. Over a 1000-tick round
/// trip, the dynamic channel keys its timeout off the RTT estimate and sends close to one
/// transmission per message. A fixed 4-tick timeout retransmits before any proof can
/// return, and so spends all its tries and gives up on a link that was working.
#[test]
fn adaptive_timeout_does_not_storm_a_high_rtt_link() {
    let messages = 16;
    let rtt = 1000;

    let adaptive = transmissions_over_rtt(Channel::new(STREAM_MSGTYPE), rtt, messages)
        .expect("the adaptive channel completes");
    let fixed_tiny = transmissions_over_rtt(
        Channel::with_params(STREAM_MSGTYPE, 8, DEFAULT_RETX_TIMEOUT),
        rtt,
        messages,
    );

    // The adaptive channel sends roughly one frame per message (a small startup burst is
    // allowed while its RTT estimate settles).
    assert!(
        adaptive < messages * 3,
        "adaptive sent {adaptive} for {messages} messages (should be near {messages})"
    );
    // The fixed tiny timeout exhausts every try of its first window before a proof lands.
    let Err((sent, error)) = fixed_tiny else {
        panic!("a 4-tick timeout over a 1000-tick round trip must give up");
    };
    assert_eq!(error, ChannelError::RetriesExhausted { sequence: 0 });
    assert_eq!(sent, 8 * usize::from(DEFAULT_MAX_TRIES));
}

#[test]
fn max_window_one_serializes_half_duplex_turns() {
    let mut channel: Channel = Channel::with_initial_rtt_and_max_window(STREAM_MSGTYPE, 5_000, 1);
    channel.send(vec![1]).expect("the send queue has room");
    channel.send(vec![2]).expect("the send queue has room");

    let first = channel.poll_transmit(0);
    assert_eq!(first.len(), 1);
    assert_eq!(channel.window(), 1);
    assert!(channel.poll_transmit(1).is_empty());

    channel.on_proof(first[0].sequence, 2);
    let second = channel.poll_transmit(2);
    assert_eq!(second.len(), 1);
    assert_eq!(channel.window(), 1);
}

/// Prove every envelope one tick after it is sent until `proofs` have landed.
fn prove_promptly<const W: usize, const Q: usize, const R: usize>(
    c: &mut Channel<W, Q, R>,
    now: &mut u64,
    proofs: u32,
) {
    let mut proven = 0u32;
    while proven < proofs {
        let envs = c.poll_transmit(*now);
        *now += 1;
        for e in envs {
            if proven < proofs {
                c.on_proof(e.sequence, *now);
                proven += 1;
            }
        }
    }
}

#[test]
fn window_shrinks_by_one_per_timeout() {
    // Grow the window into the fast tier, then let eight fresh envelopes go unproved past
    // their timeout. RNS steps the window down by one per timed-out envelope, and
    // window_max with it while it stays more than WINDOW_FLEXIBILITY above window_min.
    let mut c: Channel<64, 4096> = Channel::new(0x0001);
    for i in 0..2000u16 {
        c.send(vec![i as u8]).expect("the send queue has room");
    }
    let mut now = 0u64;
    prove_promptly(&mut c, &mut now, 200);
    // Let whatever is still in flight be proved, so only the fresh eight time out.
    for seq in 0..=u16::MAX {
        c.on_proof(seq, now);
    }
    assert_eq!(
        c.window(),
        WINDOW_MAX,
        "grew to the fast-tier ceiling first"
    );
    assert_eq!((c.window_max, c.window_min), (WINDOW_MAX, 16));

    let fresh = c.poll_transmit(now);
    assert_eq!(fresh.len(), WINDOW_MAX as usize, "a full window went out");
    let deadline = c.outstanding.values().map(|o| o.deadline).min().unwrap();
    assert!(
        c.poll_transmit(deadline - 1).is_empty(),
        "nothing before the deadline"
    );
    let resent = c.poll_transmit(deadline);
    assert_eq!(
        resent.len(),
        WINDOW_MAX as usize,
        "every envelope timed out together"
    );
    // 48 timeouts against a floor of 16: the window stops at the floor, and window_max
    // stops WINDOW_FLEXIBILITY above it.
    assert_eq!(c.window(), 16);
    assert_eq!(c.window_max, 16 + WINDOW_FLEXIBILITY);
}

#[test]
fn a_single_timeout_costs_one_window_step() {
    let mut c: Channel<64, 4096> = Channel::new(0x0001);
    for i in 0..2000u16 {
        c.send(vec![i as u8]).expect("the send queue has room");
    }
    let mut now = 0u64;
    prove_promptly(&mut c, &mut now, 200);
    for seq in 0..=u16::MAX {
        c.on_proof(seq, now);
    }
    let grown = c.window();
    let sent = c.poll_transmit(now);
    // Prove all but the first, so exactly one envelope times out.
    for e in &sent[1..] {
        c.on_proof(e.sequence, now + 1);
    }
    let window_after_proofs = c.window();
    assert_eq!(window_after_proofs, grown, "already at window_max");
    let deadline = c.outstanding.values().next().unwrap().deadline;
    let resent: Vec<u16> = c
        .poll_transmit(deadline)
        .iter()
        .map(|e| e.sequence)
        .filter(|&s| s == sent[0].sequence)
        .collect();
    assert_eq!(resent, vec![sent[0].sequence]);
    assert_eq!(c.window(), grown - 1, "one timeout, one step");
    assert_eq!(c.window_max, WINDOW_MAX - 1);
}

#[test]
fn the_window_grows_by_one_per_proof_up_to_the_slow_ceiling() {
    // A fresh channel sits in the slow tier: window 2, ceiling 5. A slow RTT never
    // promotes, so proofs open the window one at a time and stop at 5.
    let mut c: Channel = Channel::with_initial_rtt(0x0001, 1_000);
    assert_eq!((c.window(), c.window_max, c.window_min), (2, 5, 2));
    for i in 0..64u16 {
        c.send(vec![i as u8]).expect("the send queue has room");
    }
    let mut seen = Vec::new();
    let mut now = 0u64;
    for _ in 0..20 {
        let envs = c.poll_transmit(now);
        now += 1_000;
        for e in envs {
            c.on_proof(e.sequence, now);
            seen.push(c.window());
        }
    }
    assert_eq!(&seen[..4], &[3, 4, 5, 5], "one step per proof, capped at 5");
    assert_eq!(c.window(), 5, "a slow link never leaves the slow tier");
}

#[test]
fn ten_medium_rounds_promote_to_the_medium_tier() {
    // RTT 500 ticks sits between the fast and medium bounds.
    let mut c: Channel = Channel::with_initial_rtt(0x0001, 500);
    for i in 0..64u16 {
        c.send(vec![i as u8]).expect("the send queue has room");
    }
    let mut now = 0u64;
    let mut proofs = 0u32;
    while proofs < FAST_RATE_THRESHOLD - 1 {
        let envs = c.poll_transmit(now);
        now += 500;
        for e in envs {
            if proofs < FAST_RATE_THRESHOLD - 1 {
                c.on_proof(e.sequence, now);
                proofs += 1;
            }
        }
    }
    assert_eq!(c.window_max, WINDOW_MAX_SLOW, "nine rounds are not enough");
    let next = *c.outstanding.keys().next().unwrap();
    c.on_proof(next, now);
    assert_eq!(c.window_max, WINDOW_MAX_MEDIUM, "the tenth round promotes");
    assert_eq!(c.window_min, WINDOW_MIN_LIMIT_MEDIUM);
}

#[test]
fn window_grows_on_sustained_clean_proofs() {
    // A dynamic channel starts at WINDOW_INITIAL. Prove a long run of packets cleanly and
    // promptly (one-tick round trip): the RTT estimate falls into the fast tier, ten fast
    // rounds promote window_max to WINDOW_MAX, and the window climbs to it.
    let mut c: Channel<64, 4096> = Channel::new(0x0001);
    assert_eq!(c.window(), WINDOW_INITIAL, "starts at the initial window");
    for i in 0..2000u16 {
        c.send(vec![i as u8]).expect("the send queue has room");
    }
    let mut now = 0u64;
    prove_promptly(&mut c, &mut now, 2000);
    assert_eq!(c.window(), WINDOW_MAX, "climbed to the fast-tier ceiling");
}

#[test]
fn a_link_slower_than_the_slow_tier_pins_the_window_to_one() {
    let c: Channel = Channel::with_initial_rtt(0x0001, 1_451);
    assert_eq!((c.window(), c.window_max, c.window_min), (1, 1, 1));
    let c: Channel = Channel::with_initial_rtt(0x0001, 1_450);
    assert_eq!((c.window(), c.window_max, c.window_min), (2, 5, 2));
}

#[test]
fn a_link_rtt_arriving_before_any_send_reselects_the_starting_window() {
    // A responder builds its channel on a guess before the initiator's RTT packet; RNS
    // builds its channel after, so a slow link still pins the window to one.
    let mut c: Channel = Channel::with_initial_rtt(0x0001, 750);
    c.set_initial_rtt(2_000);
    assert_eq!((c.window(), c.window_max, c.window_min), (1, 1, 1));
    c.set_initial_rtt(300);
    assert_eq!((c.window(), c.window_max, c.window_min), (2, 5, 2));

    // Once something is on the wire the window is live state and is left alone.
    let mut c: Channel = Channel::with_initial_rtt(0x0001, 750);
    c.send(vec![1]).expect("the send queue has room");
    let _ = c.poll_transmit(0);
    c.set_initial_rtt(2_000);
    assert_eq!(c.window(), 2);
    assert_eq!(c.rtt, 2_000);
}

#[test]
fn retransmits_back_off_by_half_again_and_give_up_after_five_tries() {
    // One envelope, RTT 100, never proved. RNS's timeout for try n with one envelope in
    // flight is 1.5^(n-1) * max(2.5 * 100, 25) * (1 + 1.5) = 625 * 1.5^(n-1) ticks.
    let mut c: Channel = Channel::with_initial_rtt(0x0001, 100);
    c.send(vec![7]).expect("the send queue has room");
    let mut sent_at = Vec::new();
    let mut failed_at = None;
    for now in 0..20_000u64 {
        sent_at.extend(c.poll_transmit(now).iter().map(|_| now));
        if c.error().is_some() {
            failed_at = Some(now);
            break;
        }
    }
    // 625, 937, 1406, 2109, 3164 (floored).
    assert_eq!(sent_at, vec![0, 625, 1_562, 2_968, 5_077]);
    assert_eq!(failed_at, Some(8_241));
    assert_eq!(
        c.error(),
        Some(ChannelError::RetriesExhausted { sequence: 0 })
    );
    assert!(!c.send_idle(), "a failed channel is never idle");
    assert_eq!(c.send(vec![8]), Err(vec![8]), "and takes no more data");
    assert!(c.poll_transmit(100_000).is_empty(), "nor sends anything");
}

#[test]
fn more_in_flight_stretches_every_deadline() {
    // RNS raises every pending timeout when another envelope joins the ring.
    let mut c: Channel = Channel::with_initial_rtt(0x0001, 100);
    c.send(vec![1]).expect("the send queue has room");
    let _ = c.poll_transmit(0);
    assert_eq!(c.outstanding[&0].deadline, 625, "250 * (1 + 1.5)");
    c.send(vec![2]).expect("the send queue has room");
    let _ = c.poll_transmit(10);
    assert_eq!(c.outstanding[&0].deadline, 875, "250 * (2 + 1.5)");
    assert_eq!(c.outstanding[&1].deadline, 10 + 875);
}

#[test]
fn karns_rule_skips_rtt_samples_from_retransmits() {
    let mut c: Channel = Channel::with_initial_rtt(0x0001, 100);
    c.send(vec![1]).expect("the send queue has room");
    let _ = c.poll_transmit(0);
    let resent = c.poll_transmit(625);
    assert_eq!(resent.len(), 1, "retransmitted once");
    // The proof might answer either transmission, so it says nothing about the RTT.
    c.on_proof(0, 700);
    assert_eq!(c.rtt, 100, "no sample from a retransmitted envelope");

    c.set_initial_rtt(400);
    assert_eq!(c.rtt, 400, "a link measurement replaces the guess");
    c.send(vec![2]).expect("the send queue has room");
    let _ = c.poll_transmit(1_000);
    c.on_proof(1, 1_020);
    assert_eq!(
        c.rtt, 20,
        "a first transmission is sampled, replacing the guess"
    );
    c.send(vec![3]).expect("the send queue has room");
    let _ = c.poll_transmit(2_000);
    c.on_proof(2, 2_060);
    assert_eq!(c.rtt, (20 * 7 + 60) / 8, "and later samples are smoothed");
    c.set_initial_rtt(400);
    assert_eq!(c.rtt, (20 * 7 + 60) / 8, "a measured RTT is not overridden");
}
