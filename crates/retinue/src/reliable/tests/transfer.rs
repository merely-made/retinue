//! Byte transfer: lossless and lossy streams, the hash table bound, and RTT seeding.

use super::*;
use crate::channel::DEFAULT_MAX_TRIES;
use crate::lossy::LossModel;

#[cfg(feature = "compression")]
#[test]
fn queued_oversize_can_be_proved_before_decode_but_never_looks_healthy() {
    use crate::channel::{Envelope, STREAM_MSGTYPE, StreamDecodeError, StreamFrame};

    let (client, mut server) = pair_bounded::<64, 64, 256, 256, 8>(None);
    server.set_decoded_frame_limit(32).unwrap();
    let packet = |sequence: u16, frame: StreamFrame| {
        client.link.sealed_packet(
            CTX_CHANNEL,
            &Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence,
                payload: frame.encode(),
            }
            .encode(),
            &[sequence as u8 + 1; IV_LEN],
        )
    };

    let prefix = packet(
        0,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: false,
            data: b"12345678".to_vec(),
        },
    );
    assert!(server.on_data_packet(&prefix).is_some());
    let oversized = packet(
        1,
        StreamFrame {
            stream_id: 0,
            eof: true,
            compressed: true,
            data: crate::resource::compress(&alloc::vec![b'x'; 100_000]),
        },
    );
    assert!(
        server.on_data_packet(&oversized).is_some(),
        "the earlier full read queue defers decoding, so admission proves this frame"
    );
    assert_eq!(server.receive_error(), None);
    assert_eq!(server.read(), b"12345678");
    assert_eq!(
        server.receive_error(),
        Some(StreamDecodeError::DecodedFrameLimitExceeded { limit: 32 })
    );
    assert!(
        !server.recv_finished(),
        "the bad EOF cannot become a healthy EOF"
    );

    let later = packet(
        2,
        StreamFrame {
            stream_id: 0,
            eof: true,
            compressed: false,
            data: b"lost".to_vec(),
        },
    );
    assert!(
        server.on_data_packet(&later).is_none(),
        "terminal failure stops later proofs"
    );
    assert!(
        server.read().is_empty(),
        "later bytes do not reach the caller"
    );
}

/// A proof releases every hash recorded for its sequence, not only the hash it names: a
/// retransmit re-seals under a fresh IV, so one sequence reaches the wire as several hashes.
#[test]
fn a_proof_sweeps_every_hash_for_its_sequence() {
    let (mut client, mut server) = pair();
    assert_eq!(
        client.write(b"one small message"),
        b"one small message".len(),
        "the send queue took every byte"
    );
    client.finish();
    let mut ivc = 0u64;
    let mut iv = counting_iv(&mut ivc);

    // The first generation reaches the wire and is dropped on the floor, so its hashes
    // are recorded and no proof will ever name them.
    let dropped = client.poll_transmit(0, &mut iv);
    assert!(
        !dropped.is_empty(),
        "the first generation must reach the wire"
    );
    assert_eq!(client.sent.len(), dropped.len());

    // Now deliver and prove everything until the send side goes idle.
    for now in 1..2_000_000u64 {
        for packet in client.poll_transmit(now, &mut iv) {
            if let Some(proof) = server.on_data_packet(&packet) {
                client.on_proof(&proof, now);
            }
        }
        if client.send_idle() {
            break;
        }
    }

    assert!(client.send_idle(), "the stream must complete");
    assert_eq!(
        client.sent.len(),
        0,
        "hashes from the dropped generation outlived their proved sequence"
    );
    assert_eq!(
        client.unrecorded(),
        0,
        "nothing overflowed at the desktop size"
    );
}

/// A table too small for the window holds its bound, keeps putting packets on the wire,
/// and counts what it could not record.
///
/// Nothing is proved, so the table fills and stays full. Per the plan, a full table keeps
/// serving and says so; the retransmit timer carries unrecorded packets.
#[test]
fn a_full_table_holds_its_bound_and_counts_the_overflow() {
    let (mut client, _server) =
        pair_bounded::<2, 64, 256, 256, 65_536>(Some(crate::channel::WINDOW_MAX));
    assert_eq!(
        client.write(&[7u8; 4_000]),
        4_000,
        "the send queue took every byte"
    );
    client.finish();
    let mut ivc = 0u64;
    let mut iv = counting_iv(&mut ivc);

    let mut reached_the_wire = 0usize;
    for now in 0..5_000u64 {
        reached_the_wire += client.poll_transmit(now, &mut iv).len();
    }

    assert!(
        reached_the_wire > 2,
        "packets must keep reaching the wire past the table size, not stop at it"
    );
    assert!(
        client.sent.len() <= 2,
        "the table grew past its bound: {}",
        client.sent.len()
    );
    assert!(
        client.unrecorded() > 0,
        "the refusals must be counted, not silent"
    );
}

/// The board profile carries a stream end to end, running the same code the desktop
/// runs at different table sizes.
///
/// Parameterising rather than forking keeps the desktop an oracle for the board.
#[test]
fn the_small_profile_carries_a_stream_end_to_end() {
    let (mut client, mut server) = small_pair();
    let payload: Vec<u8> = (0..3_000u32).map(|i| (i.wrapping_mul(17)) as u8).collect();

    let mut offset = 0;
    let mut ivc = 0u64;
    let mut iv = counting_iv(&mut ivc);
    let mut got = Vec::new();
    let mut finished = false;

    for now in 0..200_000u64 {
        // The board's queue is shallow, so the writer feeds it as room appears. A short
        // write here is the expected case, not a failure.
        if offset < payload.len() {
            offset += client.write(&payload[offset..]);
        } else if !finished {
            finished = client.finish();
        }
        for packet in client.poll_transmit(now, &mut iv) {
            if let Some(proof) = server.on_data_packet(&packet) {
                client.on_proof(&proof, now);
            }
        }
        got.extend(server.read());
        if finished && client.send_idle() && server.recv_finished() {
            break;
        }
    }

    assert_eq!(
        got, payload,
        "the small profile must carry the bytes exactly"
    );
    assert!(client.send_idle(), "and must drain its queue");
}

/// Drive `client`'s payload to `server` over a lossy pipe on a virtual clock: channel
/// packets forward (subject to loss), proofs back (subject to loss), retransmits on the
/// clock. Asserts exact, in-order reconstruction and that the server saw eof.
fn drive_over_loss(drop_per_mille: u32, max_delay: u64, seed: u64, len: usize, tries: u8) {
    let (mut client, mut server) = pair();
    client.set_max_tries(tries);
    let payload: Vec<u8> = (0..len as u32)
        .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
        .collect();
    assert_eq!(
        client.write(&payload),
        payload.len(),
        "the send queue took every byte"
    );
    client.finish();

    let mut fwd = LossModel::new(seed)
        .drop_per_mille(drop_per_mille)
        .max_delay_ms(max_delay);
    let mut bwd = LossModel::new(seed ^ 0xABCD)
        .drop_per_mille(drop_per_mille)
        .max_delay_ms(max_delay);

    let mut to_server: Vec<(u64, Packet)> = Vec::new();
    let mut to_client: Vec<(u64, Packet)> = Vec::new();
    let mut got: Vec<u8> = Vec::new();
    let mut ivc: u64 = 0;

    for now in 0..2_000_000u64 {
        let mut iv = || {
            ivc += 1;
            let mut v = [0u8; IV_LEN];
            v[..8].copy_from_slice(&ivc.to_le_bytes());
            v
        };
        for pkt in client.poll_transmit(now, &mut iv) {
            if !fwd.should_drop() {
                to_server.push((now + 1 + fwd.delay_ms(), pkt));
            }
        }
        let mut still = Vec::new();
        for (t, pkt) in core::mem::take(&mut to_server) {
            if t <= now {
                if let Some(proof) = server.on_data_packet(&pkt)
                    && !bwd.should_drop()
                {
                    to_client.push((now + 1 + bwd.delay_ms(), proof));
                }
            } else {
                still.push((t, pkt));
            }
        }
        to_server = still;
        to_client.retain(|(t, proof)| {
            if *t <= now {
                client.on_proof(proof, now);
                false
            } else {
                true
            }
        });
        got.extend(server.read());
        if got.len() == payload.len() && client.send_idle() {
            break;
        }
    }
    assert_eq!(
        got, payload,
        "reliable stream must reconstruct exactly over loss"
    );
    assert!(server.recv_finished(), "server saw the client's eof");
    assert_eq!(client.channel_error(), None, "the sender never gave up");
    assert!(client.send_idle(), "and every packet was proved");
}

#[test]
fn reliable_stream_is_faithful_without_loss() {
    drive_over_loss(0, 0, 1, 5000, DEFAULT_MAX_TRIES);
}

/// At 30% loss each way one try fails half the time, so five tries lose about one
/// packet in thirty and RNS would usually tear a 5000-byte stream down. The limit is
/// raised to exercise retransmission at that loss; 25% below keeps RNS's five.
#[test]
fn reliable_stream_survives_drop() {
    drive_over_loss(300, 0, 7, 5000, 64);
}

#[test]
fn reliable_stream_survives_drop_reorder_and_delay() {
    drive_over_loss(250, 6, 42, 4000, DEFAULT_MAX_TRIES);
}

/// RNS's five tries would give up at 60% loss each way (a round trip succeeds 16% of
/// the time), so this raises the limit to exercise recovery at that loss.
#[test]
fn reliable_stream_survives_heavy_loss() {
    drive_over_loss(600, 3, 99, 3000, 64);
}

/// A responder times its channel by the link RTT, as RNS does: the larger of what the
/// initiator's RTT packet reports and its own proof-to-packet measurement.
#[test]
fn the_rtt_packet_seeds_the_responder_channel() {
    let (client, mut server) = pair();
    assert_eq!(server.buffer.rtt(), 750, "the medium-tier guess until told");
    let iv = [0x5a; IV_LEN];
    assert!(server.on_rtt_packet(&client.link.rtt_packet(0.3, &iv), 100));
    assert_eq!(
        server.buffer.rtt(),
        300,
        "the initiator's report, when larger"
    );
    let (client, mut server) = pair();
    assert!(server.on_rtt_packet(&client.link.rtt_packet(0.01, &iv), 120));
    assert_eq!(server.buffer.rtt(), 120, "our own measurement, when larger");
    let forged = client
        .link
        .sealed_packet(crate::link::CTX_LRRTT, b"not a float", &iv);
    assert!(
        !server.on_rtt_packet(&forged, 5),
        "anything but a float is ignored"
    );
    assert_eq!(server.buffer.rtt(), 120);

    let (client, mut server) = pair();
    assert!(server.on_rtt_packet(&client.link.rtt_packet(1e12, &iv), 10));
    assert_eq!(
        server.buffer.rtt(),
        MAX_REPORTED_RTT_MS,
        "an absurd report is capped"
    );
    assert_eq!(server.window(), 1, "and, before any send, pins the window");
}
