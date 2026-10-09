//! Buffer streams: demultiplexing, eof, compressed frames, and read bounds.

use alloc::vec;
use alloc::vec::Vec;

use crate::channel::{Buffer, Channel, Envelope, MAX_DATA_LEN, STREAM_MSGTYPE, StreamFrame};

#[test]
fn buffer_demuxes_by_stream_id_and_signals_eof() {
    // One channel carries two streams (RNS multiplexes above the sequence). A reader
    // bound to stream 5 delivers only stream 5's bytes in order, ignores stream 9,
    // and reports eof from stream 5 — not from stream 9's earlier eof.
    let mut r5: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), 8, 0, 5);
    let feed = |r: &mut Buffer, seq: u16, f: StreamFrame| {
        let _ = r.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: seq,
            payload: f.encode(),
        });
    };
    feed(
        &mut r5,
        0,
        StreamFrame {
            stream_id: 5,
            eof: false,
            compressed: false,
            data: vec![1, 2, 3],
        },
    );
    feed(
        &mut r5,
        1,
        StreamFrame {
            stream_id: 9,
            eof: false,
            compressed: false,
            data: vec![0xAA],
        },
    );
    feed(
        &mut r5,
        2,
        StreamFrame {
            stream_id: 5,
            eof: false,
            compressed: false,
            data: vec![4, 5],
        },
    );
    feed(
        &mut r5,
        3,
        StreamFrame {
            stream_id: 9,
            eof: true,
            compressed: false,
            data: vec![],
        },
    );
    assert!(!r5.recv_finished(), "stream 9's eof must not end stream 5");
    feed(
        &mut r5,
        4,
        StreamFrame {
            stream_id: 5,
            eof: true,
            compressed: false,
            data: vec![6],
        },
    );
    assert_eq!(
        r5.read_available(),
        vec![1, 2, 3, 4, 5, 6],
        "only stream 5, in order"
    );
    assert!(r5.recv_finished(), "stream 5's eof");
}

#[test]
fn an_undecodable_compressed_frame_is_terminal_after_the_prefix() {
    // Bytes flagged compressed that are not valid bz2 cannot be recovered. They must be
    // surfaced, never spliced into the stream as if they were data.
    let mut r: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), 8, 0, 0);
    let feed = |r: &mut Buffer, seq: u16, f: StreamFrame| {
        let _ = r.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: seq,
            payload: f.encode(),
        });
    };
    feed(
        &mut r,
        0,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: false,
            data: vec![1, 2],
        },
    );
    feed(
        &mut r,
        1,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: true,
            data: vec![9, 9, 9],
        },
    );
    feed(
        &mut r,
        2,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: false,
            data: vec![3],
        },
    );
    assert_eq!(
        r.read_available(),
        vec![1, 2],
        "only the prefix before the bad frame is delivered"
    );
    #[cfg(feature = "compression")]
    assert_eq!(
        r.receive_error(),
        Some(crate::channel::StreamDecodeError::InvalidCompression)
    );
    #[cfg(not(feature = "compression"))]
    assert_eq!(
        r.receive_error(),
        Some(crate::channel::StreamDecodeError::UnsupportedCompression)
    );
    assert!(!r.recv_finished(), "failure is not healthy EOF");
    assert!(r.read_available().is_empty(), "later bytes stay blocked");
}

#[test]
fn plain_frame_larger_than_read_bound_is_delivered_in_order() {
    type SmallBuffer = Buffer<64, 256, 256, 8>;
    let mut r = SmallBuffer::new();
    let data: Vec<u8> = (0..20).collect();
    assert!(
        r.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: 0,
            payload: StreamFrame {
                stream_id: 0,
                eof: true,
                compressed: false,
                data: data.clone(),
            }
            .encode(),
        })
    );

    let mut got = Vec::new();
    for _ in 0..3 {
        let chunk = r.read_available();
        assert!(chunk.len() <= 8);
        got.extend(chunk);
    }
    assert_eq!(got, data);
    assert!(r.recv_finished());
}

/// A compressed frame has already been proved to the peer by the time it reaches the
/// buffer, so dropping it would be silent loss.
#[cfg(feature = "compression")]
#[test]
fn a_compressed_frame_is_recovered_in_order() {
    let mut r: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), 8, 0, 0);
    let feed = |r: &mut Buffer, seq: u16, f: StreamFrame| {
        let _ = r.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: seq,
            payload: f.encode(),
        });
    };
    // Repetitive on purpose: bz2 only shrinks compressible input, and RNS compresses
    // only when it wins, so this is the shape that actually arrives flagged.
    let middle: Vec<u8> = std::iter::repeat_n(b'z', 512).collect();

    feed(
        &mut r,
        0,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: false,
            data: vec![1, 2],
        },
    );
    feed(
        &mut r,
        1,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: true,
            data: crate::resource::compress(&middle),
        },
    );
    feed(
        &mut r,
        2,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: false,
            data: vec![3],
        },
    );

    let mut expected = vec![1, 2];
    expected.extend_from_slice(&middle);
    expected.push(3);
    assert_eq!(r.read_available(), expected, "recovered, and in wire order");
    assert!(
        !r.had_unsupported_frame(),
        "a frame this build can decode is not unsupported",
    );
}

/// One bz2 frame can expand beyond the read queue's capacity. Keep later frames
/// behind it and deliver every byte across repeated bounded reads.
#[cfg(feature = "compression")]
#[test]
fn expanded_frame_respects_read_bound_without_losing_following_data() {
    type SmallBuffer = Buffer<64, 256, 256, 8>;
    let mut r = SmallBuffer::new();
    let middle = vec![b'z'; 40];
    for (sequence, frame) in [
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: true,
            data: crate::resource::compress(&middle),
        },
        StreamFrame {
            stream_id: 0,
            eof: true,
            compressed: false,
            data: vec![1, 2, 3],
        },
    ]
    .into_iter()
    .enumerate()
    {
        assert!(r.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: sequence as u16,
            payload: frame.encode(),
        }));
    }

    let mut got = Vec::new();
    let mut first = [0u8; 3];
    assert_eq!(r.read(&mut first), first.len());
    got.extend_from_slice(&first);
    assert!(r.read_buf.len() <= 8);
    assert!(
        !r.recv_finished(),
        "eof follows the pending compressed data"
    );

    for _ in 0..10 {
        let chunk = r.read_available();
        assert!(chunk.len() <= 8, "one read exceeded READ_BYTES");
        assert!(
            r.read_buf.len() <= 8,
            "the queued bytes exceeded READ_BYTES"
        );
        got.extend(chunk);
        if got.len() == middle.len() + 3 {
            break;
        }
    }
    let mut expected = middle;
    expected.extend_from_slice(&[1, 2, 3]);
    assert_eq!(
        got, expected,
        "the expanded frame and EOF frame stay in order"
    );
    assert!(r.recv_finished());
    assert!(!r.had_unsupported_frame());
}

#[cfg(feature = "compression")]
#[test]
fn oversized_compressed_frame_stops_before_eof_and_bounds_output_allocation() {
    type SmallBuffer = Buffer<64, 256, 256, 8>;
    let mut r = SmallBuffer::new();
    assert_eq!(
        r.set_decoded_frame_limit(0),
        Err(crate::channel::StreamDecodeLimitError::InvalidLimit)
    );
    assert_eq!(
        r.set_decoded_frame_limit(usize::MAX),
        Err(crate::channel::StreamDecodeLimitError::InvalidLimit)
    );
    r.set_decoded_frame_limit(32).unwrap();
    let expanded = vec![b'x'; 100_000];
    let compressed = crate::resource::compress(&expanded);
    assert!(
        compressed.len() < 200,
        "small wire frame expands far past the ceiling"
    );
    let err = crate::resource::decompress_bounded(&compressed, 32).unwrap_err();
    assert_eq!(err, crate::resource::BoundedDecompressError::LimitExceeded);
    let exact =
        crate::resource::decompress_bounded(&crate::resource::compress(&expanded[..32]), 32)
            .unwrap();
    assert_eq!(exact.len(), 32);
    assert_eq!(
        exact.capacity(),
        33,
        "owned output allocation includes one sentinel byte"
    );

    for (sequence, frame) in [
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: false,
            data: vec![1, 2],
        },
        StreamFrame {
            stream_id: 0,
            eof: true,
            compressed: true,
            data: compressed,
        },
        StreamFrame {
            stream_id: 0,
            eof: true,
            compressed: false,
            data: vec![3],
        },
    ]
    .into_iter()
    .enumerate()
    {
        assert!(r.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: sequence as u16,
            payload: frame.encode()
        }));
    }
    assert_eq!(r.read_available(), vec![1, 2]);
    assert_eq!(
        r.receive_error(),
        Some(crate::channel::StreamDecodeError::DecodedFrameLimitExceeded { limit: 32 })
    );
    assert!(!r.recv_finished());
    assert!(r.read_available().is_empty());
}

#[cfg(feature = "compression")]
#[test]
fn eof_on_expanded_frame_waits_for_all_bytes_to_be_read() {
    type SmallBuffer = Buffer<64, 256, 256, 8>;
    let mut r = SmallBuffer::new();
    let data = vec![b'x'; 20];
    assert!(
        r.handle(Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: 0,
            payload: StreamFrame {
                stream_id: 0,
                eof: true,
                compressed: true,
                data: crate::resource::compress(&data),
            }
            .encode(),
        })
    );

    assert!(!r.recv_finished(), "eof must wait behind buffered data");
    let mut got = Vec::new();
    for _ in 0..3 {
        let chunk = r.read_available();
        assert!(chunk.len() <= 8);
        got.extend(chunk);
        if got.len() < data.len() {
            assert!(!r.recv_finished(), "pending bytes must precede eof");
        }
    }
    assert_eq!(got, data);
    assert!(r.recv_finished(), "eof follows the last read byte");
}

#[test]
fn buffer_stream_round_trips_with_finish() {
    // The everyday path: write a payload and finish() over the lossless proof model;
    // the reader reconstructs it exactly and sees eof.
    let mut tx: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), MAX_DATA_LEN, 3, 3);
    let mut rx: Buffer = Buffer::with_streams(Channel::new(STREAM_MSGTYPE), MAX_DATA_LEN, 3, 3);
    let payload: Vec<u8> = (0..2000u32).map(|i| (i * 7 + 1) as u8).collect();
    assert_eq!(
        tx.write(&payload),
        payload.len(),
        "the send queue took every byte"
    );
    assert!(tx.finish(), "the send queue had room for eof");
    let mut got = Vec::new();
    for now in 0..100_000u64 {
        let envs = tx.poll_transmit(now);
        if envs.is_empty() && tx.send_idle() {
            break;
        }
        for e in envs {
            let seq = e.sequence;
            let _ = rx.handle(e);
            tx.on_proof(seq, now);
        }
        got.extend(rx.read_available());
    }
    got.extend(rx.read_available());
    assert_eq!(got, payload, "stream reconstructs exactly");
    assert!(rx.recv_finished(), "reader saw the writer's eof");
}

/// A Buffer reads only stream messages (review #6). Another message type is
/// sequenced and proved, so the stream moves past it, but its bytes are never read as
/// a stream frame: an `80 00` payload would otherwise be an eof frame on stream 0, and
/// `40 00` a compressed frame that fails to decode and tears the stream down.
#[test]
fn buffer_ignores_messages_that_are_not_stream_data() {
    let mut rx: Buffer = Buffer::new();
    let foreign = |sequence: u16, payload: &[u8]| Envelope {
        msgtype: 0x0101,
        sequence,
        payload: payload.to_vec(),
    };
    assert!(
        rx.handle(foreign(0, &[0x80, 0x00, b'x', b'y'])),
        "a foreign message is still proved"
    );
    assert!(rx.handle(foreign(1, &[0x40, 0x00, 0xde, 0xad])));
    assert!(rx.read_available().is_empty(), "no bytes reach the reader");
    assert!(!rx.recv_finished(), "a foreign message cannot set eof");
    assert_eq!(rx.receive_error(), None, "nor raise a receive error");

    // The sequence moved past both, so the next stream frame delivers in order.
    let frame = StreamFrame {
        stream_id: 0,
        eof: true,
        compressed: false,
        data: b"ok".to_vec(),
    };
    assert!(rx.handle(Envelope {
        msgtype: STREAM_MSGTYPE,
        sequence: 2,
        payload: frame.encode(),
    }));
    assert_eq!(rx.read_available(), b"ok".to_vec());
    assert!(rx.recv_finished(), "the stream's own eof still counts");
}
