//! Reliable stream drivers and their failure modes.

use super::*;

#[cfg(feature = "compression")]
#[tokio::test]
async fn oversized_reliable_frame_closes_without_proof_and_errors_at_host_reader() {
    use crate::channel::{Envelope, STREAM_MSGTYPE, StreamFrame};
    use crate::link::{PendingLink, accept};

    let server_id = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let endpoint = Endpoint::new(server_id.clone());
    endpoint.set_reliable_decoded_frame_limit(32).unwrap();
    let mut iface = endpoint.attach_interface();
    let dest =
        DestinationName::new("retinue", ["decode-limit"]).destination_hash(server_id.public());
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x63; 64], trailer);
    let (server_link, proof) = accept(&request, &server_id, &[0x64; 64], trailer).unwrap();
    let client_link = pending.prove(&proof).unwrap();
    let mut stream = register_reliable_stream(
        &endpoint.shared,
        server_link,
        iface.id(),
        Liveness::responder(0, 0),
        None,
        LinkDirection::Inbound,
        LinkRemoteFact {
            destination: Some(dest),
            identity: Some(*client_id.public()),
        },
    )
    .unwrap();

    let wire_frame = |sequence: u16, frame: StreamFrame| {
        client_link.sealed_packet(
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
    let prefix = wire_frame(
        0,
        StreamFrame {
            stream_id: 0,
            eof: false,
            compressed: false,
            data: b"prefix".to_vec(),
        },
    );
    let oversized = wire_frame(
        1,
        StreamFrame {
            stream_id: 0,
            eof: true,
            compressed: true,
            data: crate::resource::compress(&vec![b'x'; 100_000]),
        },
    );
    let sink = iface.sink();
    assert!(sink.deliver(prefix));
    assert!(sink.deliver(oversized));

    let mut got = Vec::new();
    let err = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut got))
        .await
        .expect("driver closes failed stream")
        .unwrap_err();
    assert_eq!(got, b"prefix", "already decoded bytes reach the caller");
    assert_eq!(
        err.kind(),
        io::ErrorKind::InvalidData,
        "no healthy EOF after decode failure"
    );

    let mut proof_count = 0;
    loop {
        let packet = tokio::time::timeout(Duration::from_secs(2), iface.next_outbound())
            .await
            .expect("driver emits a close packet")
            .unwrap();
        if packet.context == CTX_LINKCLOSE {
            break;
        }
        if packet.packet_type == PacketType::Proof {
            proof_count += 1;
        }
    }
    assert_eq!(proof_count, 1, "the oversized frame itself is not proved");
}

/// A LINKCLOSE is the peer's only if it decrypts to the link id. One with the right
/// context and address but a garbage payload is anybody's, and must not end the stream;
/// the genuine close still does.
#[tokio::test]
async fn a_forged_link_close_leaves_a_reliable_stream_open() {
    use crate::channel::{Envelope, STREAM_MSGTYPE, StreamFrame};
    use crate::link::{PendingLink, accept};

    let server_id = PrivateIdentity::from_secret_bytes(&[0x71; 64]);
    let endpoint = Endpoint::new(server_id.clone());
    let iface = endpoint.attach_interface();
    let dest =
        DestinationName::new("retinue", ["forged-close"]).destination_hash(server_id.public());
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x73; 64], trailer);
    let (server_link, proof) = accept(&request, &server_id, &[0x74; 64], trailer).unwrap();
    let client_link = pending.prove(&proof).unwrap();
    let mut stream = register_reliable_stream(
        &endpoint.shared,
        server_link,
        iface.id(),
        Liveness::responder(0, 0),
        None,
        LinkDirection::Inbound,
        LinkRemoteFact {
            destination: Some(dest),
            identity: None,
        },
    )
    .unwrap();
    let sink = iface.sink();

    let forged = client_link.framed_packet(CTX_LINKCLOSE, vec![0xA5; 48]);
    assert!(sink.deliver(forged));
    let frame = client_link.sealed_packet(
        CTX_CHANNEL,
        &Envelope {
            msgtype: STREAM_MSGTYPE,
            sequence: 0,
            payload: StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: false,
                data: b"still open".to_vec(),
            }
            .encode(),
        }
        .encode(),
        &[0x01; IV_LEN],
    );
    assert!(sink.deliver(frame));
    let mut got = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut got))
        .await
        .expect("the stream still delivers")
        .expect("the forged close did not end the stream");
    assert_eq!(&got, b"still open");

    assert!(sink.deliver(client_link.close_packet(&[0x02; IV_LEN])));
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut rest))
        .await
        .expect("a genuine close ends the stream")
        .unwrap();
    assert!(rest.is_empty());
}

#[tokio::test]
async fn reliable_read_keeps_prefix_then_wakes_with_decode_error() {
    let (mine, mut driver_half) = tokio::io::duplex(64);
    let error = Arc::new(Mutex::new(None));
    let mut stream = LinkStream {
        inner: mine,
        receive_error: Some(Arc::clone(&error)),
        lost: Arc::default(),
        link_id: AddressHash::from_bytes([7; 16]),
        iface: 0,
    };
    driver_half.write_all(b"prefix").await.unwrap();
    let (pending_tx, pending_rx) = oneshot::channel();
    let reader = tokio::spawn(async move {
        let mut got = Vec::new();
        let mut pending_tx = Some(pending_tx);
        let mut chunk = [0u8; 64];
        loop {
            let result: io::Result<usize> = std::future::poll_fn(|cx| {
                let mut read_buf = ReadBuf::new(&mut chunk);
                match Pin::new(&mut stream).poll_read(cx, &mut read_buf) {
                    Poll::Pending => {
                        if let Some(tx) = pending_tx.take() {
                            let _ = tx.send(());
                        }
                        Poll::Pending
                    }
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
                    Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                }
            })
            .await;
            match result {
                Ok(0) => break (Ok(()), got),
                Ok(n) => got.extend_from_slice(&chunk[..n]),
                Err(error) => break (Err(error), got),
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(1), pending_rx)
        .await
        .expect("reader reaches Pending after prefix")
        .unwrap();
    *error.lock().unwrap() =
        Some(StreamDecodeError::DecodedFrameLimitExceeded { limit: 32 }.into());
    drop(driver_half); // closing the duplex half wakes an already-pending reader
    let (result, got) = tokio::time::timeout(Duration::from_secs(1), reader)
        .await
        .expect("pending reader woke")
        .unwrap();
    assert_eq!(got, b"prefix");
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
}
