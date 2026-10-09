use alloc::string::ToString;

use super::proof::identify_signed_message;
use super::*;
use crate::Error;
use crate::destination::DestinationName;
use crate::hash::AddressHash;
use crate::identity::{IDENTITY_LEN, Identity, KEY_LEN, PrivateIdentity};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use crate::token::IV_LEN;

fn request(dest_hex: &str, payload_hex: &str) -> Packet {
    let mut dest = [0u8; 16];
    hex::decode_to_slice(dest_hex, &mut dest).unwrap();
    Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::LinkRequest,
        hops: 0,
        transport: None,
        destination: AddressHash::from_bytes(dest),
        context: 0,
        payload: hex::decode(payload_hex).unwrap(),
    }
}

/// Captured: RNS proved back to this link id for a 64-byte request retinue sent it.
#[test]
fn link_id_matches_the_captured_proof_address() {
    let p = request(
        "a8725a7e212dace39e9f99a8ac5da28c",
        "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
         a09aa5f47a6759802ff955f8dc2d2a14a5c99d23be97f864127ff9383455a4f0",
    );
    assert_eq!(
        link_id(&p).unwrap().to_string(),
        "7c88505173382e78aaaae5ecdf122eec",
    );
}

/// Captured: RNS reported this link id for the 67-byte request it sent retinue. The
/// trailer must NOT feed the hash, and this is the case that proves it.
#[test]
fn link_id_ignores_the_trailer() {
    let p = request(
        "19208507854a8a0b871f881170d475aa",
        "f72075eeade493f3a3fd94d98cba8b628cf5cce2532b0903b48b1c2024676164\
         f7cf9beb5793e668eb8c589096d382c616db7a7ebdd0ec6407e8a7d89452dd4e\
         202000",
    );
    assert_eq!(p.payload.len(), LINK_REQUEST_LEN);
    assert_eq!(
        link_id(&p).unwrap().to_string(),
        "5452ae4c3251cffa6b080779f943dfa4",
    );
}

#[test]
fn captured_trailers_decode() {
    // What an RNS initiator sends: AES-256, asking for 8192.
    let req = LinkTrailer::decode(&[0x20, 0x20, 0x00]).unwrap();
    assert_eq!(req.mode, LinkMode::Aes256Cbc);
    assert_eq!(req.mtu, 8192);

    // What the responder answers: AES-256, settling on 500 (= Reticulum.MTU).
    let proof = LinkTrailer::decode(&[0x20, 0x01, 0xf4]).unwrap();
    assert_eq!(proof.mode, LinkMode::Aes256Cbc);
    assert_eq!(proof.mtu, 500);
}

#[test]
fn trailers_round_trip() {
    for t in [
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 8192,
        },
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
        LinkTrailer {
            mode: LinkMode::Aes128Cbc,
            mtu: 500,
        },
    ] {
        assert_eq!(LinkTrailer::decode(&t.encode()).unwrap(), t);
    }
    assert_eq!(
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 8192
        }
        .encode(),
        [0x20, 0x20, 0x00],
    );
}

#[test]
fn a_short_payload_is_an_error() {
    let p = request("a8725a7e212dace39e9f99a8ac5da28c", "0faa");
    assert!(link_id(&p).is_err());
}

/// Initiator and responder, both retinue, must agree on the link id and the session
/// key, and then talk. This checks the two sides are mutually consistent; the oracle
/// gates check each side against RNS.
#[test]
fn initiator_and_responder_agree() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let peer = *dest_identity.public();
    let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);

    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };

    // Initiator opens.
    let (pending, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], trailer);

    // Responder accepts and proves.
    let (responder_link, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();

    // Initiator verifies the proof and establishes.
    let initiator_link = pending.prove(&proof).unwrap();

    assert_eq!(initiator_link.id(), responder_link.id());
    assert_eq!(initiator_link.id(), pending.link_id());

    // The shared key round-trips: what one encrypts, the other decrypts.
    let msg = b"across the link";
    let packet = initiator_link.data_packet(msg, &[0x01; 16]);
    assert_eq!(
        responder_link.receive(&packet),
        Some(Inbound::Data(msg.to_vec()))
    );

    let back = responder_link.data_packet(b"and back", &[0x02; 16]);
    assert_eq!(
        initiator_link.receive(&back),
        Some(Inbound::Data(b"and back".to_vec())),
    );
}

#[test]
fn receive_classifies_link_traffic() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let (_pending, request) = PendingLink::open(
        DestinationName::new("retinue", ["test"]).destination_hash(dest_identity.public()),
        *dest_identity.public(),
        &[0x33; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    let (link, _proof) = accept(
        &request,
        &dest_identity,
        &[0x99; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    )
    .unwrap();

    assert_eq!(
        link.receive(&link.keepalive_packet(KEEPALIVE_REQUEST)),
        Some(Inbound::KeepAliveRequest),
    );
    assert_eq!(
        link.receive(&link.keepalive_packet(KEEPALIVE_RESPONSE)),
        Some(Inbound::KeepAliveResponse),
    );
    let close = link.close_packet(&[0x44; IV_LEN]);
    assert_eq!(link.receive(&close), Some(Inbound::Close));
    let mut malformed_close = close;
    malformed_close.payload.truncate(16);
    assert_eq!(link.receive(&malformed_close), Some(Inbound::Unknown));

    // A packet for a different link id is not ours.
    let mut foreign = link.keepalive_packet(KEEPALIVE_REQUEST);
    foreign.destination = AddressHash::from_bytes([0xAB; 16]);
    assert_eq!(link.receive(&foreign), None);
}

/// Link-data proofs use RNS's keys in both directions: the initiator signs with the
/// ephemeral key from its request, the responder with its destination identity, and
/// each side validates the other's with no IDENTIFY.
#[test]
fn link_data_proofs_use_the_link_keys() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let (pending, request) = PendingLink::open(
        DestinationName::new("retinue", ["test"]).destination_hash(dest_identity.public()),
        *dest_identity.public(),
        &[0x33; 64],
        trailer,
    );
    let (responder, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();
    let initiator = pending.prove(&proof).unwrap();
    let ephemeral = PrivateIdentity::from_secret_bytes(&[0x33; 64]);
    assert_eq!(
        &request.payload[KEY_LEN..LINK_KEYS_LEN],
        ephemeral.public().ed25519_bytes(),
        "request bytes 32..64 are the ephemeral Ed25519 key"
    );

    let to_responder = initiator.data_packet(b"up", &[0x01; IV_LEN]);
    let up = initiator.prove_packet(&to_responder);
    assert_eq!(
        up.encode(),
        data_proof_packet(initiator.id(), &to_responder.full_hash(), &ephemeral).encode(),
        "the initiator signs with its ephemeral key"
    );
    assert_eq!(
        responder.validate_proof(&up),
        Some(to_responder.full_hash())
    );

    let to_initiator = responder.data_packet(b"down", &[0x02; IV_LEN]);
    let down = responder.prove_packet(&to_initiator);
    assert_eq!(
        down.encode(),
        data_proof_packet(responder.id(), &to_initiator.full_hash(), &dest_identity).encode(),
        "the responder signs with its identity"
    );
    assert_eq!(
        initiator.validate_proof(&down),
        Some(to_initiator.full_hash())
    );

    // Neither side takes its own proof, or a stranger's, for the peer's.
    assert_eq!(initiator.validate_proof(&up), None);
    assert_eq!(responder.validate_proof(&down), None);
    let stranger = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
    assert_eq!(
        responder.validate_proof(&initiator.data_proof(&to_responder, &stranger)),
        None
    );
}

/// Gold test: retinue builds the exact link-data proof RNS 1.3.8 emitted for a packet
/// we sent it (captured in `rns_link_proof.json`), and validates RNS's own proof back.
/// Ed25519 is deterministic, so signing the same full hash with the same identity
/// reproduces RNS's signature byte for byte — this pins the whole proof wire.
#[test]
fn data_proof_matches_rns_capture() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/rns_link_proof.json")).unwrap();
    let link_id =
        AddressHash::from_slice(&hex::decode(doc["link_id_hex"].as_str().unwrap()).unwrap())
            .unwrap();
    let full_hash: [u8; 32] = hex::decode(doc["our_sent_packet_hash_full_hex"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let seed: [u8; 64] = hex::decode(doc["prover_identity_secret_seed_hex"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let prover = PrivateIdentity::from_secret_bytes(&seed);

    // Generate: our proof packet is RNS's wire bytes exactly.
    let proof = data_proof_packet(link_id, &full_hash, &prover);
    assert_eq!(
        hex::encode(proof.encode()),
        doc["proofs"][0]["frame_hex"].as_str().unwrap(),
        "proof packet must equal RNS's wire bytes",
    );

    // Validate: retinue accepts RNS's proof against RNS's identity, recovering the hash.
    let pubbytes: [u8; 64] = hex::decode(doc["prover_identity_public_hex"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let peer = Identity::from_public_bytes(&pubbytes).unwrap();
    assert_eq!(
        read_data_proof(link_id, &proof, &peer),
        Some(full_hash),
        "validates and recovers the proven hash",
    );

    // A tampered signature is rejected, and a proof for a different link is not ours.
    let mut bad = proof.clone();
    *bad.payload.last_mut().unwrap() ^= 0x01;
    assert_eq!(
        read_data_proof(link_id, &bad, &peer),
        None,
        "tamper rejected"
    );
    assert_eq!(
        read_data_proof(AddressHash::from_bytes([0x00; 16]), &proof, &peer),
        None,
        "wrong link rejected",
    );
}

/// Gold test: retinue signs a link IDENTIFY to RNS 1.3.8's exact signature (captured in
/// link_identify.json), which also confirms retinue's identity-pubkey derivation matches.
#[test]
fn identify_signature_matches_rns_capture() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/link_identify.json")).unwrap();
    let link_id =
        AddressHash::from_slice(&hex::decode(doc["link_id_hex"].as_str().unwrap()).unwrap())
            .unwrap();
    let public: [u8; IDENTITY_LEN] = hex::decode(doc["our_identity_public_hex"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let seed: [u8; IDENTITY_LEN] =
        hex::decode(doc["our_identity_secret_seed_hex"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
    let me = PrivateIdentity::from_secret_bytes(&seed);
    assert_eq!(
        me.public().to_public_bytes(),
        public,
        "pubkey derivation matches RNS"
    );
    let sig = me.sign(&identify_signed_message(link_id, &public));
    assert_eq!(
        hex::encode(sig),
        doc["signature_hex"].as_str().unwrap(),
        "identify signature must equal RNS's",
    );
}

/// An initiator's identify round-trips over a real link: the responder recovers the
/// initiator's identity, and a tampered packet is rejected.
#[test]
fn identify_round_trips_over_a_link() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let (pending, request) = PendingLink::open(
        DestinationName::new("retinue", ["test"]).destination_hash(dest_identity.public()),
        *dest_identity.public(),
        &[0x33; 64],
        trailer,
    );
    let (responder, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();
    let initiator = pending.prove(&proof).unwrap();

    let me = PrivateIdentity::from_secret_bytes(&[0x77; 64]);
    let pkt = initiator.identify_packet(&me, &[0x01; 16]);
    let learned = responder.read_identify(&pkt).expect("valid identify");
    assert_eq!(
        learned.hash(),
        me.public().hash(),
        "responder learns the initiator"
    );

    let mut bad = pkt.clone();
    *bad.payload.last_mut().unwrap() ^= 0x01;
    assert!(
        responder.read_identify(&bad).is_none(),
        "tampered identify rejected"
    );
}

/// A transport hop or destination lowers the signalled MTU without changing the link id,
/// and leaves a bare request or a smaller request alone.
#[test]
fn clamping_a_request_keeps_its_link_id() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let peer = *dest_identity.public();
    let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
    let asked = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 8192,
    };
    let (pending, mut request) = PendingLink::open(dest_hash, peer, &[0x33; 64], asked);

    clamp_request_mtu(&mut request, 500).unwrap();
    assert_eq!(link_id(&request).unwrap(), pending.link_id());
    assert_eq!(
        request_trailer(&request).unwrap(),
        Some(LinkTrailer { mtu: 500, ..asked })
    );
    clamp_request_mtu(&mut request, 1024).unwrap();
    assert_eq!(request_trailer(&request).unwrap().unwrap().mtu, 500);

    let mut bare = request.clone();
    bare.payload.truncate(LINK_KEYS_LEN);
    clamp_request_mtu(&mut bare, 255).unwrap();
    assert_eq!(bare.payload.len(), LINK_KEYS_LEN);
    assert_eq!(request_trailer(&bare).unwrap(), None);

    let mut undecodable = request.clone();
    undecodable.payload[LINK_KEYS_LEN] = 0xe0;
    let before = undecodable.payload.clone();
    clamp_request_mtu(&mut undecodable, 500).expect("a fitting request passes unread");
    assert_eq!(undecodable.payload, before);
    assert!(clamp_request_mtu(&mut undecodable, 255).is_err());
}

/// The initiator holds the link to the MTU it asked for, whatever the proof echoes.
#[test]
fn an_initiator_never_adopts_more_than_it_requested() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let peer = *dest_identity.public();
    let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
    let asked = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 255,
    };
    let (pending, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], asked);
    let (_, proof) = accept(
        &request,
        &dest_identity,
        &[0x99; 64],
        LinkTrailer { mtu: 500, ..asked },
    )
    .unwrap();
    assert_eq!(pending.prove(&proof).unwrap().mtu(), 255);

    let (_, lower) = accept(
        &request,
        &dest_identity,
        &[0x99; 64],
        LinkTrailer { mtu: 247, ..asked },
    )
    .unwrap();
    assert_eq!(pending.prove(&lower).unwrap().mtu(), 247);
}

/// A relay accepts only the destination's own signature over the proof, trailer included.
#[test]
fn a_transport_hop_checks_the_proof_signature() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let impostor = PrivateIdentity::from_secret_bytes(&[0x12; 64]);
    let peer = *dest_identity.public();
    let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let (_, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], trailer);
    let (_, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();
    assert!(proof_is_signed_by(&proof, &peer));
    assert!(!proof_is_signed_by(&proof, impostor.public()));

    let mut raised = proof.clone();
    raised.payload[LINK_PROOF_LEN - 1] ^= 1;
    assert!(!proof_is_signed_by(&raised, &peer), "the trailer is signed");

    let (_, forged) = accept(&request, &impostor, &[0x99; 64], trailer).unwrap();
    assert!(!proof_is_signed_by(&forged, &peer));

    let mut data = proof;
    data.context = 0;
    assert!(!proof_is_signed_by(&data, &peer));
}

/// A request is exactly 64 or 67 bytes (`Link.py` 187) and signals an enabled mode that the
/// proof echoes (`Link.py` 149, 225-227, 367).
#[test]
fn a_responder_refuses_malformed_requests_and_disabled_modes() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let peer = *dest_identity.public();
    let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let (_, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], trailer);
    let respond = |request: &Packet, offered: LinkTrailer| {
        accept(request, &dest_identity, &[0x99; 64], offered).map(|_| ())
    };
    assert_eq!(respond(&request, trailer), Ok(()));

    let mut bare = request.clone();
    bare.payload.truncate(LINK_KEYS_LEN);
    assert_eq!(respond(&bare, trailer), Ok(()), "no trailer means AES-256");
    for (len, error) in [
        (LINK_KEYS_LEN - 1, Error::Truncated),
        (LINK_KEYS_LEN + 1, Error::NotALinkRequest),
        (LINK_REQUEST_LEN + 1, Error::NotALinkRequest),
    ] {
        let mut odd = request.clone();
        odd.payload.resize(len, 0);
        assert_eq!(respond(&odd, trailer), Err(error), "{len} bytes");
    }

    let aes128 = LinkTrailer {
        mode: LinkMode::Aes128Cbc,
        ..trailer
    };
    let mut disabled = request.clone();
    disabled.payload[LINK_KEYS_LEN..].copy_from_slice(&aes128.encode());
    assert_eq!(respond(&disabled, aes128), Err(Error::BadLinkMode));
    assert_eq!(respond(&request, aes128), Err(Error::BadLinkMode));
}

/// A proof is exactly 96 or 99 bytes and signals the mode that was requested
/// (`Link.py` 396-405).
#[test]
fn an_initiator_refuses_malformed_proofs_and_other_modes() {
    let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let peer = *dest_identity.public();
    let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: 500,
    };
    let (pending, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], trailer);
    let (_, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();
    assert!(pending.prove(&proof).is_ok());

    // Without a trailer the proof signs the keys alone and signals the default mode.
    let mut bare = proof.clone();
    bare.payload.truncate(LINK_PROOF_LEN - TRAILER_LEN);
    let mut signed = pending.link_id().as_slice().to_vec();
    signed.extend_from_slice(&bare.payload[64..]);
    signed.extend_from_slice(peer.ed25519_bytes());
    bare.payload[..64].copy_from_slice(&dest_identity.sign(&signed));
    assert_eq!(pending.prove(&bare).unwrap().mtu(), 500);

    for (len, error) in [
        (LINK_PROOF_LEN - TRAILER_LEN - 1, Error::Truncated),
        (LINK_PROOF_LEN - 1, Error::NotAProof),
        (LINK_PROOF_LEN + 1, Error::NotAProof),
    ] {
        let mut odd = proof.clone();
        odd.payload.resize(len, 0);
        assert_eq!(pending.prove(&odd).err(), Some(error), "{len} bytes");
    }

    let mut other_mode = proof;
    other_mode.payload[LINK_PROOF_LEN - TRAILER_LEN..].copy_from_slice(
        &LinkTrailer {
            mode: LinkMode::Aes128Cbc,
            ..trailer
        }
        .encode(),
    );
    assert_eq!(pending.prove(&other_mode).err(), Some(Error::BadLinkMode));
}
