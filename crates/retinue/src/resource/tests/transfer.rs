use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::resource::*;

#[test]
fn outgoing_hmu_is_bounded_and_idempotent() {
    let data = vec![0xA5; 64];
    let mut token = vec![0x5A; SDU * (HASHMAP_MAX_PARTS * 3 + 1)];
    for (index, part) in token.chunks_mut(SDU).enumerate() {
        part[..4].copy_from_slice(&(index as u32).to_be_bytes());
    }
    let mut outgoing = Outgoing::new(&data, &token, [1, 2, 3, 4], false);
    let advertisement = outgoing.advertisement();
    let last_advertised: [u8; MAPHASH_LEN] = *advertisement
        .hashmap
        .as_chunks::<MAPHASH_LEN>()
        .0
        .last()
        .unwrap();

    let first = outgoing.hmu_after(&last_advertised);
    let first_hmu = parse_hmu(&first).unwrap();
    assert_eq!(first_hmu.segment, 1);
    assert_eq!(first_hmu.hashes.len(), HASHMAP_MAX_PARTS);
    assert_eq!(outgoing.hmu_after(&last_advertised), first);

    let second_hmu = parse_hmu(
        outgoing
            .hmu_after(first_hmu.hashes.last().unwrap())
            .as_slice(),
    )
    .unwrap();
    assert_eq!(second_hmu.segment, 2);
    assert_eq!(second_hmu.hashes.len(), HASHMAP_MAX_PARTS);
}

/// A peer chooses the advertised part count, and the wire field is a `u64`. Without a
/// ceiling this node holds reassembly state for a resource the peer simply made up.
#[test]
fn an_advertisement_claiming_more_parts_than_the_limit_is_refused() {
    let data: Vec<u8> = (0..4_000u32).map(|i| i as u8).collect();
    let rh = [0xAB, 0xCD, 0xEF, 0x01];
    let out = Outgoing::new(&data, &data, rh, false);

    let honest = out.advertisement();
    let claimed = honest.parts;
    assert!(
        claimed > 1,
        "the fixture needs more than one part to be a real test"
    );

    // The honest advertisement is accepted at a limit that covers it, and refused at one
    // that does not. The node returns a typed error rather than allocating.
    assert!(Incoming::new_with_max_parts(&honest, claimed as usize).is_ok());
    assert_eq!(
        Incoming::new_with_max_parts(&honest, (claimed - 1) as usize).err(),
        Some(Error::CapacityExceeded)
    );

    // An outright lie is refused by the default ceiling.
    let mut liar = out.advertisement();
    liar.parts = u64::MAX;
    assert_eq!(
        Incoming::new(&liar).err(),
        Some(Error::CapacityExceeded),
        "a peer must not be able to claim an unbounded resource"
    );
}

/// A >74-part resource round-trips in-process through the windowed HMU path: advertise,
/// request windows, solicit + ingest HMUs, serve, reassemble.
#[test]
fn windowed_sender_receiver_round_trip() {
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;
    use crate::link::{LinkMode, LinkTrailer, PendingLink, accept};

    let dest_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let (pending, req) = PendingLink::open(
        DestinationName::new("retinue", ["r"]).destination_hash(dest_id.public()),
        *dest_id.public(),
        &[0x33; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    let (recv_link, proof_pkt) = accept(
        &req,
        &dest_id,
        &[0x99; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    )
    .unwrap();
    let send_link = pending.prove(&proof_pkt).unwrap();

    // ~120 parts of data.
    let data: Vec<u8> = (0..55_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 8) as u8)
        .collect();
    let rh = [0xAB, 0xCD, 0xEF, 0x01];
    let token = send_link.seal(&content(&data, &rh), &[7u8; 16]);
    let mut out = Outgoing::new(&data, &token, rh, false);
    assert!(out.total_parts() > HASHMAP_MAX_PARTS);

    let mut inc = Incoming::new(&out.advertisement()).unwrap();
    // Drive the windowed exchange to completion.
    loop {
        if inc.is_complete() {
            break;
        }
        let want = inc.missing_known();
        if !want.is_empty() {
            let req = parse_request(&inc.request(&want)).unwrap();
            for part in out.serve(&req) {
                inc.accept_part(&part);
            }
        } else if inc.needs_hmu() {
            let solicit = parse_request(&inc.solicit_hmu()).unwrap();
            let last = solicit.last_map_hash.unwrap();
            let hmu = parse_hmu(&out.hmu_after(&last)).unwrap();
            assert!(inc.ingest_hmu(&hmu) > 0);
        } else {
            panic!("stuck: not complete, nothing to request, no HMU needed");
        }
    }
    let recovered = inc
        .recover(&recv_link.open(&inc.assemble_token().unwrap()).unwrap())
        .unwrap();
    assert_eq!(recovered, data);
    assert_eq!(inc.proof(&recovered), out.expected_proof());
}

/// A multi-segment resource round-trips sender -> receiver in-process: two segments
/// sharing one original_hash, driven windowed, recovered bodies concatenated.
#[test]
fn multi_segment_round_trip() {
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;
    use crate::link::{LinkMode, LinkTrailer, PendingLink, accept};

    let dest_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let (pending, req) = PendingLink::open(
        DestinationName::new("retinue", ["r"]).destination_hash(dest_id.public()),
        *dest_id.public(),
        &[0x33; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    let (recv_link, proof_pkt) = accept(
        &req,
        &dest_id,
        &[0x99; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    )
    .unwrap();
    let send_link = pending.prove(&proof_pkt).unwrap();

    // Two segments of ~90 parts each (small "MAX_SEGMENT_SIZE" for the test).
    const SEG: usize = 40_000;
    let data: Vec<u8> = (0..90_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 8) as u8)
        .collect();
    let mut original_hash = None;

    let mut assembled = Vec::new();
    let total_segs = data.chunks(SEG).count() as i64;
    for (idx, chunk) in data.chunks(SEG).enumerate() {
        let rh = [(idx as u8) + 1, 2, 3, 4];
        let token = send_link.seal(&content(chunk, &rh), &[7u8; 16]);
        let out = Outgoing::new(chunk, &token, rh, false);
        // The shared identity is the first segment's hash as built.
        let original = *original_hash.get_or_insert(out.resource_hash());
        let mut out = out.with_segment(idx as i64 + 1, total_segs, data.len() as u64, original);
        // Check the advertisement carries the shared identity and total size.
        let adv = out.advertisement();
        assert_eq!(adv.original_hash, original.to_vec());
        assert_eq!(adv.data_size, data.len() as u64);

        let mut inc = Incoming::new(&adv).unwrap();
        loop {
            if inc.is_complete() {
                break;
            }
            let want = inc.missing_known();
            if !want.is_empty() {
                let r = parse_request(&inc.request(&want)).unwrap();
                for part in out.serve(&r) {
                    inc.accept_part(&part);
                }
            } else {
                let s = parse_request(&inc.solicit_hmu()).unwrap();
                let hmu = parse_hmu(&out.hmu_after(&s.last_map_hash.unwrap())).unwrap();
                inc.ingest_hmu(&hmu);
            }
        }
        let body = inc
            .recover(&recv_link.open(&inc.assemble_token().unwrap()).unwrap())
            .unwrap();
        assembled.extend_from_slice(&body);
    }
    assert_eq!(assembled, data);
}

/// Two different 3-byte parts whose map hashes collide under random hash `01020304`
/// (`7f83d108`), found by a birthday search.
const COLLIDING: [[u8; 3]; 2] = [[0x00, 0x5d, 0x3c], [0x00, 0xf7, 0x30]];
const COLLIDING_SALT: [u8; RANDOM_HASH_LEN] = [1, 2, 3, 4];

/// Drive `out` into `inc` to completion, returning the reassembled token.
fn drive(out: &mut Outgoing, inc: &mut Incoming) -> Vec<u8> {
    for _ in 0..10_000 {
        if inc.is_complete() {
            return inc.assemble_token().unwrap();
        }
        let want = inc.missing_known();
        if !want.is_empty() {
            let request = parse_request(&inc.request(&want)).unwrap();
            for part in out.serve(&request) {
                inc.accept_part(&part);
            }
        } else {
            assert!(
                inc.needs_hmu(),
                "stuck: nothing to request and no HMU needed"
            );
            let solicit = parse_request(&inc.solicit_hmu()).unwrap();
            let hmu = parse_hmu(&out.hmu_after(&solicit.last_map_hash.unwrap())).unwrap();
            assert!(inc.ingest_hmu(&hmu) > 0, "the HMU added hashes");
        }
    }
    panic!("the transfer did not complete");
}

#[test]
fn the_collision_fixture_collides() {
    assert_eq!(
        map_hash(&COLLIDING[0], &COLLIDING_SALT),
        map_hash(&COLLIDING[1], &COLLIDING_SALT)
    );
    assert_ne!(COLLIDING[0], COLLIDING[1]);
}

/// A receiver stores parts by position, so two parts sharing a map hash both land, in
/// order.
#[test]
fn colliding_map_hashes_are_placed_by_position() {
    let token = COLLIDING.concat();
    let (_, hashmap) = split_parts_with_size(&token, &COLLIDING_SALT, 3);
    let advertisement = Advertisement {
        transfer_size: token.len() as u64,
        data_size: 1,
        parts: 2,
        resource_hash: vec![0; 32],
        original_hash: vec![0; 32],
        random_hash: COLLIDING_SALT.to_vec(),
        flags: FLAG_ENCRYPTED,
        hashmap,
        i: 1,
        l: 1,
        q: None,
    };
    let mut inc = Incoming::new(&advertisement).unwrap();
    assert_eq!(inc.missing_known().len(), 2, "both are asked for");
    assert!(inc.accept_part(&COLLIDING[0]));
    assert!(inc.accept_part(&COLLIDING[1]));
    assert!(!inc.accept_part(&COLLIDING[1]), "and neither twice");
    assert!(inc.is_complete());
    assert_eq!(inc.assemble_token().unwrap(), token);
}

/// A sender whose random hash makes two different parts share a map hash draws another,
/// as RNS does, so every request names exactly one part.
#[test]
fn a_sender_redraws_a_colliding_random_hash() {
    let data = b"data";
    let token = COLLIDING.concat();
    let mut out = Outgoing::from_token(data, token.clone(), COLLIDING_SALT, false, 3);
    assert_ne!(out.random_hash(), COLLIDING_SALT, "re-drawn");
    let advertisement = out.advertisement();
    let hashes = advertisement.hashmap.as_chunks::<MAPHASH_LEN>().0;
    assert_ne!(hashes[0], hashes[1]);
    assert_eq!(
        out.resource_hash(),
        resource_hash(data, &out.random_hash()),
        "the resource hash follows the new random hash"
    );
    assert_eq!(out.expected_proof(), proof(data, &out.resource_hash()));
    for (index, hash) in hashes.iter().enumerate() {
        let request = parse_request(&build_request(&out.resource_hash(), &[*hash])).unwrap();
        assert_eq!(out.serve(&request), vec![COLLIDING[index].to_vec()]);
    }
    let mut inc = Incoming::new(&advertisement).unwrap();
    assert_eq!(drive(&mut out, &mut inc), token);
}

/// The first segment's identity is its own hash, whatever the caller computed up front:
/// a random hash re-drawn on a map-hash collision changes the resource hash, and a stale
/// `original_hash` would keep RNS from grouping the segments.
#[test]
fn the_first_segment_is_its_own_identity() {
    let token = vec![7_u8; SDU * 3];
    let out = Outgoing::new(b"data", &token, [5, 6, 7, 8], false);
    let hash = out.resource_hash();
    let first = out.with_segment(1, 2, 8, [0; 32]).advertisement();
    assert_eq!(first.original_hash, hash.to_vec());
    let second = Outgoing::new(b"more", &token, [9, 6, 7, 8], false)
        .with_segment(2, 2, 8, hash)
        .advertisement();
    assert_eq!(second.original_hash, hash.to_vec());
}

/// Byte-identical parts in different hashmap segments share a map hash harmlessly.
#[test]
fn identical_parts_in_different_hashmap_segments_complete() {
    let mut token = vec![0_u8; SDU * (HASHMAP_MAX_PARTS + 20)];
    for (index, part) in token.chunks_mut(SDU).enumerate() {
        // Part HASHMAP_MAX_PARTS + 6 repeats part 3; every other part is distinct.
        let tag = if index == HASHMAP_MAX_PARTS + 6 {
            3
        } else {
            index
        };
        part[..4].copy_from_slice(&(tag as u32).to_be_bytes());
    }
    let mut out = Outgoing::new(b"data", &token, [5, 6, 7, 8], false);
    assert_eq!(
        out.random_hash(),
        [5, 6, 7, 8],
        "identical parts need no re-draw"
    );
    assert_eq!(
        out.copies_of(3).collect::<Vec<_>>(),
        [3, HASHMAP_MAX_PARTS + 6],
        "either copy serves both slots"
    );
    let mut inc = Incoming::new(&out.advertisement()).unwrap();
    assert_eq!(drive(&mut out, &mut inc), token);
}

/// An HMU lands at `segment * 74`, as RNS places it, whatever order HMUs arrive in.
#[test]
fn hashmap_updates_land_by_segment() {
    let mut token = vec![0_u8; SDU * (HASHMAP_MAX_PARTS * 2 + 30)];
    for (index, part) in token.chunks_mut(SDU).enumerate() {
        part[..4].copy_from_slice(&(index as u32).to_be_bytes());
    }
    let mut out = Outgoing::new(b"data", &token, [5, 6, 7, 8], false);
    let advertisement = out.advertisement();
    let mut inc = Incoming::new(&advertisement).unwrap();
    let map: Vec<[u8; MAPHASH_LEN]> = token
        .chunks(SDU)
        .map(|part| map_hash(part, &out.random_hash()))
        .collect();
    let second = parse_hmu(&out.hmu_after(&map[2 * HASHMAP_MAX_PARTS - 1])).unwrap();
    let first = parse_hmu(&out.hmu_after(&map[HASHMAP_MAX_PARTS - 1])).unwrap();
    assert_eq!((first.segment, second.segment), (1, 2));
    assert_eq!(inc.ingest_hmu(&second), 30);
    assert_eq!(inc.ingest_hmu(&first), HASHMAP_MAX_PARTS);
    assert_eq!(inc.ingest_hmu(&first), 0, "a repeated HMU changes nothing");
    assert!(inc.have_all_hashes());
    assert_eq!(drive(&mut out, &mut inc), token);
}

/// A part is matched only within the window after the first missing part, so a part
/// that arrives early, past the window, is not taken.
#[test]
fn parts_are_matched_within_the_window() {
    let mut token = vec![0_u8; SDU * 8];
    for (index, part) in token.chunks_mut(SDU).enumerate() {
        part[..4].copy_from_slice(&(index as u32).to_be_bytes());
    }
    let out = Outgoing::new(b"data", &token, [5, 6, 7, 8], false);
    let mut inc = Incoming::new(&out.advertisement()).unwrap().with_window(2);
    assert_eq!(inc.missing_known().len(), 2);
    assert!(!inc.accept_part(out.part(5).unwrap()), "past the window");
    assert!(inc.accept_part(out.part(1).unwrap()));
    assert!(inc.accept_part(out.part(0).unwrap()));
    assert_eq!(inc.missing_known().len(), 2, "the window moved past both");
    assert!(inc.accept_part(out.part(2).unwrap()));
}

/// The sender and receiver halves agree end to end, through a real AES token: build an
/// advertisement and parts, then receive them back and recover the payload. This mirrors
/// the live RNS gate without needing RNS.
#[test]
fn sender_and_receiver_round_trip() {
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;
    use crate::link::{LinkMode, LinkTrailer, PendingLink, accept};

    let dest_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let (pending, req) = PendingLink::open(
        DestinationName::new("retinue", ["r"]).destination_hash(dest_id.public()),
        *dest_id.public(),
        &[0x33; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    let (recv_link, proof_pkt) = accept(
        &req,
        &dest_id,
        &[0x99; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    )
    .unwrap();
    let send_link = pending.prove(&proof_pkt).unwrap();

    // Sender: content = rh || data, sealed, split, advertised.
    let data: Vec<u8> = (0..1000u32).map(|i| (i * 3 + 1) as u8).collect();
    let rh = [0xAB, 0xCD, 0xEF, 0x01];
    let token = send_link.seal(&content(&data, &rh), &[7u8; 16]);
    let (adv, parts) = advertise(&data, &token, rh, false);

    // Receiver: parse, collect parts, recover.
    let mut inc = Incoming::new(&adv).unwrap();
    for p in &parts {
        assert!(inc.accept_part(p));
    }
    assert!(inc.is_complete());
    let recovered_content = recv_link.open(&inc.assemble_token().unwrap()).unwrap();
    let recovered = data_from_content(&recovered_content).unwrap();
    assert_eq!(recovered, &data[..]);
    assert!(inc.verify(recovered));
    // Proof round-trips to the value the sender precomputed.
    assert_eq!(inc.proof(recovered), proof(&data, &inc.resource_hash()));
}

/// Parts land in place in a token sized from the advertisement. A last part that arrives
/// first waits until another fixes the part size, and a part of the wrong size is refused.
#[test]
fn parts_assemble_in_place_in_any_order() {
    let token: Vec<u8> = (0..250_u32).map(|i| i as u8).collect();
    let out = Outgoing::from_token(b"data", token.clone(), [4, 3, 2, 1], false, 100);
    let mut inc = Incoming::new(&out.advertisement()).unwrap();
    let part = |i: usize| out.part(i).unwrap();
    assert!(inc.accept_part(part(2)), "the 50-byte tail is held aside");
    assert!(!inc.is_complete());
    assert!(inc.accept_part(part(0)));
    assert!(inc.accept_part(part(1)));
    assert!(inc.is_complete());
    assert_eq!(inc.take_token().unwrap(), token);

    let mut inc = Incoming::new(&out.advertisement()).unwrap();
    let mut short = part(0).to_vec();
    short.pop();
    assert!(!inc.accept_part(&short), "an unknown hash");
    assert!(inc.accept_part(part(0)));
    assert!(inc.take_token().is_err(), "incomplete");
}

/// The next request asks for every known missing part in the window, and says when the
/// window reaches past the known hashmap.
#[test]
fn the_next_request_marks_an_exhausted_hashmap() {
    let data = vec![0xA5; 64];
    let token = vec![0x5A; SDU * (HASHMAP_MAX_PARTS + 4)];
    let out = Outgoing::new(&data, &token, [1, 2, 3, 4], false);
    let mut inc = Incoming::new(&out.advertisement()).unwrap();
    inc.set_window(WINDOW_MAX);
    let (wanted, exhausted) = inc.next_request();
    assert!(exhausted);
    assert_eq!(wanted.len(), HASHMAP_MAX_PARTS);
    inc.set_window(10);
    assert_eq!(inc.next_request(), (wanted[..10].to_vec(), false));
}
