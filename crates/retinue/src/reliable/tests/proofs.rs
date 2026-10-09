//! Proof keys and peer identity: which proofs release a packet, and IDENTIFY handling.

use super::*;

#[test]
fn a_forged_proof_releases_nothing() {
    // A proof signed by the wrong identity, or naming a packet we never sent, must not
    // release an outstanding sequence.
    let (mut client, mut server) = pair();
    assert_eq!(
        client.write(b"one small message that fits in a single channel packet"),
        b"one small message that fits in a single channel packet".len(),
        "the send queue took every byte"
    );
    let mut ivc = 0u64;
    let mut iv = || {
        ivc += 1;
        let mut v = [0u8; IV_LEN];
        v[..8].copy_from_slice(&ivc.to_le_bytes());
        v
    };
    let sent = client.poll_transmit(0, &mut iv);
    assert!(!sent.is_empty());
    server.on_data_packet(&sent[0]).unwrap();

    // A proof from a stranger's identity over the right hash: rejected (wrong signer).
    let stranger = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
    let forged = client.link.data_proof(&sent[0], &stranger);
    assert!(
        !client.on_proof(&forged, 1),
        "wrong-identity proof rejected"
    );
    assert!(!client.send_idle(), "the packet is still outstanding");

    // The genuine proof (server signs with its identity) does release it.
    let real = server.on_data_packet(&sent[0]).unwrap();
    assert!(client.on_proof(&real, 2), "genuine proof accepted");
}

/// An RNS initiator proves link data with the ephemeral key from its link request and
/// never needs to IDENTIFY for that. A responder must take the key from the request, or
/// nothing it sends to an RNS peer is ever released.
#[test]
fn a_responder_releases_an_ephemeral_proof_without_identify() {
    let (_client, mut server, _request, _) = unidentified_pair();
    let sent = server_sends(&mut server);

    // Built as RNS builds it: the explicit proof, signed by the initiator's ephemeral
    // seed (the one `PendingLink::open` was given).
    let ephemeral = PrivateIdentity::from_secret_bytes(&[0x33; 64]);
    let proof = crate::link::data_proof_packet(server.link_id(), &sent.full_hash(), &ephemeral);

    assert!(server.peer().is_none(), "the initiator never identified");
    assert!(
        server.on_proof(&proof, 1),
        "the ephemeral proof is accepted"
    );
    assert!(server.send_idle(), "the server's packet is released");
}

/// The initiator proves with its ephemeral key, which the responder reads out of the
/// request, and not with its long-term identity.
#[test]
fn an_initiator_proves_with_its_ephemeral_key() {
    let (mut client, mut server, request, client_id) = unidentified_pair();
    let sent = server_sends(&mut server);
    let proof = client
        .on_data_packet(&sent)
        .expect("client proves the server's packet");

    let request_key = Identity::from_public_bytes(&request.payload[..64].try_into().unwrap())
        .expect("request keys parse");
    let link_id = client.link_id();
    assert_eq!(
        crate::link::read_data_proof(link_id, &proof, &request_key),
        Some(sent.full_hash()),
        "signed by the key in request bytes 32..64"
    );
    assert_eq!(
        crate::link::read_data_proof(link_id, &proof, client_id.public()),
        None,
        "not signed by the initiator's long-term identity"
    );
    assert!(server.on_proof(&proof, 1), "the responder releases it");
}

/// The responder proves with its destination identity, which an initiator knows from
/// the announce.
#[test]
fn a_responder_proves_with_its_identity() {
    let (mut client, mut server, _request, _) = unidentified_pair();
    assert_eq!(client.write(b"to the server"), 13);
    let mut ivc = 0u64;
    let sent = client.poll_transmit(0, counting_iv(&mut ivc));
    let proof = server.on_data_packet(&sent[0]).expect("server proves");
    let server_pub = *PrivateIdentity::from_secret_bytes(&[0x22; 64]).public();
    assert_eq!(
        crate::link::read_data_proof(client.link_id(), &proof, &server_pub),
        Some(sent[0].full_hash())
    );
    assert!(client.on_proof(&proof, 1), "the initiator releases it");
}

/// Transitional: an older retinue initiator proves with its long-term identity. A
/// responder accepts that once the initiator has identified, and not before.
#[test]
fn a_responder_accepts_a_long_term_proof_after_identify() {
    let (client, mut server, _request, client_id) = unidentified_pair();
    let sent = server_sends(&mut server);
    let legacy = client.link.data_proof(&sent, &client_id);

    assert!(
        !server.on_proof(&legacy, 1),
        "a long-term proof needs an identity to check it against"
    );
    assert!(!server.send_idle());

    let id_packet = client.link.identify_packet(&client_id, &[0x07; IV_LEN]);
    assert!(server.on_identify(&id_packet), "server learns the client");
    assert_eq!(
        server.peer().map(|p| p.hash()),
        Some(client_id.public().hash())
    );
    assert!(server.on_proof(&legacy, 2), "accepted after identify");
    assert!(server.send_idle(), "the server's packet is now released");
}

/// A relay on a shared medium echoes the initiator's own IDENTIFY, validly signed under the
/// link key. Fed straight in (bypassing the router's filter), the channel must refuse it, or
/// the initiator becomes its own peer and every real proof fails.
#[test]
fn an_echoed_own_identify_does_not_become_the_peer() {
    let (mut client, mut server) = pair();
    let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let server_hash = server.prover.public().hash();

    let mut echo = client.link.identify_packet(&client_id, &[0x07; IV_LEN]);
    echo.hops += 1;
    let adopted = client.on_identify(&echo);
    let peer_after = client.peer().map(|p| p.hash());

    assert_eq!(client.write(b"after the echo"), b"after the echo".len());
    let mut ivc = 0u64;
    let sent = client.poll_transmit(0, counting_iv(&mut ivc));
    let proof = server
        .on_data_packet(&sent[0])
        .expect("the server proves the client's packet");
    let proof_accepted = client.on_proof(&proof, 1);

    assert_eq!(
        (adopted, peer_after == Some(server_hash), proof_accepted),
        (false, true, true),
        "(echo adopted, peer is still the server, server's proof accepted); \
         peer after the echo: {peer_after:?}, client is {:?}",
        client_id.public().hash()
    );
}

/// Positive controls for the guard: a responder still learns the initiator from its first
/// IDENTIFY, and the link carries data and proofs both ways afterwards. Its own identity,
/// a second identity, and a repeat of the first, verbatim or re-sealed, are each refused
/// without disturbing that.
#[test]
fn a_responder_learns_its_first_peer_only_and_the_link_still_works() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let stranger = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let dest = DestinationName::new("retinue", ["test"]).destination_hash(server_id.public());
    let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x33; 64], trailer);
    let (responder_link, proof) = accept(&request, &server_id, &[0x99; 64], trailer).unwrap();
    let initiator_link = pending.prove(&proof).unwrap();
    let mut client: ReliableChannel =
        ReliableChannel::new(initiator_link, client_id.clone(), *server_id.public());
    let mut server: ReliableChannel = ReliableChannel::accepting(responder_link, server_id.clone());
    let client_hash = Some(client_id.public().hash());

    let own = server.link.identify_packet(&server_id, &[0x01; IV_LEN]);
    assert!(!server.on_identify(&own), "its own identity is refused");
    assert!(server.peer().is_none());

    let genuine = client.link.identify_packet(&client_id, &[0x02; IV_LEN]);
    assert!(
        server.on_identify(&genuine),
        "the first IDENTIFY is learned"
    );
    assert_eq!(server.peer().map(|p| p.hash()), client_hash);

    let other = client.link.identify_packet(&stranger, &[0x03; IV_LEN]);
    assert!(!server.on_identify(&other), "a second identity is refused");
    assert!(!server.on_identify(&genuine), "a repeat learns nothing new");
    // The initiator re-sends under a fresh IV (Ruling 72): a new packet, the same identity.
    let resend = client.link.identify_packet(&client_id, &[0x04; IV_LEN]);
    assert_ne!(resend.hash(), genuine.hash(), "a re-send is a new packet");
    assert!(
        !server.on_identify(&resend),
        "a fresh-IV re-send learns nothing new"
    );
    assert_eq!(server.peer().map(|p| p.hash()), client_hash);

    let mut ivc = 0u64;
    assert_eq!(client.write(b"to the server"), 13);
    let sent = client.poll_transmit(0, counting_iv(&mut ivc));
    let ack = server.on_data_packet(&sent[0]).expect("server proves");
    assert!(client.on_proof(&ack, 1), "the client's packet is released");
    assert_eq!(server.read(), b"to the server");

    assert_eq!(server.write(b"to the client"), 13);
    let sent = server.poll_transmit(2, counting_iv(&mut ivc));
    let ack = client.on_data_packet(&sent[0]).expect("client proves");
    assert!(server.on_proof(&ack, 3), "the server's packet is released");
    assert_eq!(client.read(), b"to the client");
    assert!(client.send_idle() && server.send_idle());
}
