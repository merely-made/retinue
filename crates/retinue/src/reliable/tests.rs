//! ReliableChannel tests and their link fixtures.

use super::*;
use crate::capacity::small_types::SmallReliableChannel;
use crate::destination::DestinationName;
use crate::link::{LinkMode, LinkTrailer, PendingLink, accept};

mod proofs;
mod transfer;

/// A client (initiator) and server (responder) reliable channel over one established
/// link, each holding the other's identity for proof validation.
fn pair() -> (ReliableChannel, ReliableChannel) {
    pair_bounded(None)
}

/// The same pair at a caller-chosen table size and send window, for exercising a full
/// table. The window matters: it starts at [`crate::channel::WINDOW_INITIAL`] and only
/// opens on sustained proofs, so a short transfer finishes before enough packets are
/// outstanding to fill anything.
fn pair_bounded<const N: usize, const W: usize, const Q: usize, const R: usize, const B: usize>(
    max_window: Option<u32>,
) -> (
    ReliableChannel<N, W, Q, R, B>,
    ReliableChannel<N, W, Q, R, B>,
) {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let dest = DestinationName::new("retinue", ["test"]).destination_hash(server_id.public());
    let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x33; 64], trailer);
    let (responder_link, proof) = accept(&request, &server_id, &[0x99; 64], trailer).unwrap();
    let initiator_link = pending.prove(&proof).unwrap();

    match max_window {
        None => (
            ReliableChannel::new(initiator_link, client_id.clone(), *server_id.public()),
            ReliableChannel::new(responder_link, server_id, *client_id.public()),
        ),
        Some(window) => (
            ReliableChannel::new_with_initial_rtt_and_max_window(
                initiator_link,
                client_id.clone(),
                *server_id.public(),
                10,
                window,
            ),
            ReliableChannel::new_with_initial_rtt_and_max_window(
                responder_link,
                server_id,
                *client_id.public(),
                10,
                window,
            ),
        ),
    }
}

/// The same pair at the board profile. Inference picks the parameters off the return
/// type, so the small profile is named once, in `capacity`.
fn small_pair() -> (SmallReliableChannel, SmallReliableChannel) {
    pair_bounded(None)
}

fn counting_iv(counter: &mut u64) -> impl FnMut() -> [u8; IV_LEN] + '_ {
    move || {
        *counter += 1;
        let mut v = [0u8; IV_LEN];
        v[..8].copy_from_slice(&counter.to_le_bytes());
        v
    }
}

/// Initiator and responder channels over one link, as an endpoint builds them: the
/// responder does not know the initiator. Also returns the link request, whose bytes
/// 32..64 are the initiator's ephemeral Ed25519 key, and the initiator's identity.
fn unidentified_pair() -> (ReliableChannel, ReliableChannel, Packet, PrivateIdentity) {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let dest = DestinationName::new("retinue", ["test"]).destination_hash(server_id.public());
    let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x33; 64], trailer);
    let (responder_link, proof) = accept(&request, &server_id, &[0x99; 64], trailer).unwrap();
    let initiator_link = pending.prove(&proof).unwrap();
    let server_pub = *server_id.public();
    (
        ReliableChannel::new(initiator_link, client_id.clone(), server_pub),
        ReliableChannel::accepting(responder_link, server_id),
        request,
        client_id,
    )
}

/// One message from the server, as the packet it put on the wire.
fn server_sends(server: &mut ReliableChannel) -> Packet {
    assert_eq!(server.write(b"from the server"), 15);
    let mut ivc = 0u64;
    let mut sent = server.poll_transmit(0, counting_iv(&mut ivc));
    assert_eq!(sent.len(), 1);
    sent.remove(0)
}
