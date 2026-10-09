//! In-order delivery: lossless, over loss, across the sequence wrap, and through reorder.

use alloc::vec;
use alloc::vec::Vec;

use crate::channel::{Buffer, Channel, DEFAULT_MAX_TRIES, Envelope, STREAM_MSGTYPE};
use crate::lossy::LossModel;

#[test]
fn lossless_in_order_delivery() {
    let mut tx: Channel = Channel::new(0xABCD);
    let mut rx: Channel = Channel::new(0xABCD);
    for i in 0u8..20 {
        tx.send(vec![i]).expect("the send queue has room");
    }
    let mut got = Vec::new();
    for now in 0..1000 {
        for e in tx.poll_transmit(now) {
            let seq = e.sequence;
            let _ = rx.handle(e);
            tx.on_proof(seq, now); // lossless: every packet is immediately proven
        }
        while let Some(m) = rx.recv() {
            got.push(m[0]);
        }
        if got.len() == 20 {
            break;
        }
    }
    assert_eq!(got, (0u8..20).collect::<Vec<_>>());
}

/// Run a byte stream through two Buffers across a deterministic lossy pipe on a virtual
/// clock, proving each delivered envelope back (subject to loss). Asserts exact reconstruction.
fn stream_over_loss(drop_per_mille: u32, max_delay_ticks: u64, seed: u64, max_tries: u8) {
    let payload: Vec<u8> = (0..4000u32)
        .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
        .collect();
    let mut tx: Buffer = Buffer::new();
    let mut rx: Buffer = Buffer::new();
    tx.set_max_tries(max_tries);
    assert_eq!(
        tx.write(&payload),
        payload.len(),
        "the send queue took every byte"
    );

    let mut fwd = LossModel::new(seed)
        .drop_per_mille(drop_per_mille)
        .max_delay_ms(max_delay_ticks);
    let mut bwd = LossModel::new(seed ^ 0xFFFF)
        .drop_per_mille(drop_per_mille)
        .max_delay_ms(max_delay_ticks);

    // In flight: (arrival_tick, item). Forward carries envelopes; back carries the
    // sequence of a proof (the link auto-proves every received packet).
    let mut to_rx: Vec<(u64, Envelope)> = Vec::new();
    let mut to_tx: Vec<(u64, u16)> = Vec::new();
    let mut got: Vec<u8> = Vec::new();

    for now in 0..1_000_000u64 {
        for e in tx.poll_transmit(now) {
            if !fwd.should_drop() {
                to_rx.push((now + 1 + fwd.delay_ms(), e));
            }
        }
        // Deliver due envelopes; prove each one back (dup or not).
        let mut still = Vec::new();
        for (t, e) in core::mem::take(&mut to_rx) {
            if t <= now {
                let seq = e.sequence;
                let _ = rx.handle(e);
                if !bwd.should_drop() {
                    to_tx.push((now + 1 + bwd.delay_ms(), seq));
                }
            } else {
                still.push((t, e));
            }
        }
        to_rx = still;
        to_tx.retain(|(t, seq)| {
            if *t <= now {
                tx.on_proof(*seq, now);
                false
            } else {
                true
            }
        });
        got.extend(rx.read_available());
        if got.len() == payload.len() && tx.send_idle() {
            break;
        }
    }
    assert_eq!(got, payload, "stream must reconstruct exactly over loss");
    assert_eq!(tx.channel_error(), None, "the sender never gave up");
    assert!(tx.send_idle(), "and every envelope was proved");
}

/// At 30% loss each way one try fails half the time, so RNS's five tries lose about
/// one envelope in thirty; this raises the limit to exercise retransmission at that loss.
#[test]
fn stream_survives_drop() {
    stream_over_loss(300, 0, 11, 64);
}

#[test]
fn stream_survives_drop_reorder_and_delay() {
    stream_over_loss(250, 6, 99, DEFAULT_MAX_TRIES);
}

/// At 60% loss each way a round trip succeeds 16% of the time, so RNS's five tries
/// would give up on most envelopes. This exercises reordering and retransmission at
/// that loss, so it raises the try limit.
#[test]
fn heavy_loss_still_converges() {
    stream_over_loss(600, 3, 7, 64);
}

#[test]
fn sequence_wraps_past_the_16bit_modulus() {
    // Push more than 65536 messages so the sequence wraps, and confirm order holds
    // across the wrap. Small window keeps it quick.
    let mut tx: Channel = Channel::with_params(0x0001, 4, 2);
    let mut rx: Channel = Channel::with_params(0x0001, 4, 2);
    let total = 70_000u32; // > SEQ_MODULUS
    let mut sent = 0u32;
    let mut got = 0u32;
    for now in 0..5_000_000u64 {
        while sent < total && tx.in_flight() < 4 && tx.send_room() > 0 {
            tx.send(vec![(sent % 251) as u8])
                .expect("the send queue has room");
            sent += 1;
        }
        for e in tx.poll_transmit(now) {
            let seq = e.sequence;
            let _ = rx.handle(e);
            tx.on_proof(seq, now);
        }
        while let Some(m) = rx.recv() {
            assert_eq!(m, vec![(got % 251) as u8], "in order across the wrap");
            got += 1;
        }
        if got == total {
            break;
        }
    }
    assert_eq!(got, total, "all delivered across the sequence wrap");
}

/// A proved frame must never strand in the reorder buffer (review finding, 2026-07-31).
///
/// A buffered frame is proved on arrival and never retransmitted. If the inbox fills
/// mid-drain, only the read path can deliver the frame left in `reorder`.
#[test]
fn a_proved_frame_never_strands_when_the_inbox_fills_mid_drain() {
    let env = |seq: u16| Envelope {
        msgtype: 0x0001,
        sequence: seq,
        payload: vec![seq as u8],
    };
    // QUEUE = 2: the inbox holds two frames, so delivering seq 0 with seqs 1 and 2
    // already buffered fills it mid-drain and leaves seq 2 behind in reorder.
    let mut rx: Channel<8, 2, 8> = Channel::with_params(0x0001, 4, 2);
    assert!(rx.handle(env(1)), "future frame is buffered and proved");
    assert!(
        rx.handle(env(2)),
        "second future frame is buffered and proved"
    );
    assert!(rx.handle(env(0)), "the gap frame is delivered and proved");

    // The sender got proofs for all three, so nothing will ever be retransmitted.
    // Reading must still deliver every frame in order.
    assert_eq!(rx.recv().as_deref(), Some(&[0u8][..]));
    assert_eq!(rx.recv().as_deref(), Some(&[1u8][..]));
    assert_eq!(
        rx.recv().as_deref(),
        Some(&[2u8][..]),
        "the frame the full inbox left in reorder must surface once the app makes room"
    );
    assert_eq!(rx.recv(), None, "and nothing further is owed");
}

#[test]
fn channel_reports_each_message_type() {
    let mut rx: Channel = Channel::new(STREAM_MSGTYPE);
    for (sequence, msgtype) in [(1u16, 0x0101u16), (0, STREAM_MSGTYPE)] {
        assert!(rx.handle(Envelope {
            msgtype,
            sequence,
            payload: vec![sequence as u8],
        }));
    }
    assert_eq!(rx.recv_message(), Some((STREAM_MSGTYPE, vec![0])));
    assert_eq!(rx.recv_message(), Some((0x0101, vec![1])));
    assert_eq!(rx.recv_message(), None);
}
