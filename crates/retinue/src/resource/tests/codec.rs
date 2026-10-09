use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::resource::*;

/// The real advertisement captured in `oracle/capture_resource.py`.
const ADV: &str = "8ba174cd02d0a164cd1000a16e02a168c42011b44f89a2dc4d73865701b5174b2\
                   0a532c53325651383a749b33c863b7fb60ea172c404fddb2d74a16fc42011b44f\
                   89a2dc4d73865701b5174b20a532c53325651383a749b33c863b7fb60ea16901a\
                   16c01a171c0a16603a16dc408202ecd18fe3e1fcb";

fn adv_bytes() -> Vec<u8> {
    hex::decode(ADV.replace([' ', '\n'], "")).unwrap()
}

#[test]
fn initial_hashmap_cannot_exceed_advertised_part_count() {
    let mut adv = Advertisement::parse(&adv_bytes()).unwrap();
    adv.parts = 1;
    assert!(matches!(
        Incoming::new_with_max_parts(&adv, 32),
        Err(Error::BadRequest)
    ));
    adv.parts = 2;
    adv.hashmap.push(0);
    assert!(matches!(
        Incoming::new_with_max_parts(&adv, 32),
        Err(Error::BadRequest)
    ));
}

#[test]
fn hmu_cannot_grow_past_the_advertised_count() {
    let adv = Advertisement::parse(&adv_bytes()).unwrap();
    let mut incoming = Incoming::new_with_max_parts(&adv, 32).unwrap();
    let hmu = parse_hmu(&build_hmu(
        &incoming.resource_hash(),
        1,
        &[[9; MAPHASH_LEN]],
    ))
    .unwrap();
    assert_eq!(incoming.ingest_hmu(&hmu), 0);
    assert_eq!(incoming.order_len(), 2);
}

#[test]
fn parses_the_captured_advertisement() {
    let a = Advertisement::parse(&adv_bytes()).unwrap();
    assert_eq!(a.transfer_size, 720);
    assert_eq!(a.data_size, 4096);
    assert_eq!(a.parts, 2);
    assert_eq!(a.flags, 3);
    assert_eq!(a.resource_hash.len(), 32);
    assert_eq!(a.original_hash.len(), 32);
    assert_eq!(a.random_hash.len(), 4);
    assert_eq!(a.random_hash, hex::decode("fddb2d74").unwrap());
    assert_eq!(a.hashmap, hex::decode("202ecd18fe3e1fcb").unwrap());
    assert_eq!(a.hashmap_parts(), 2);
    assert_eq!(a.i, 1);
    assert_eq!(a.l, 1);
    assert_eq!(a.q, None);
}

#[test]
fn parses_a_stock_large_response_advertisement() {
    let bytes = hex::decode(
        "8ba174cd1130a164cd10f7a16e0aa168c420896316a0df053d734e4cd8e3083fc5cb8a73d3dd3785841629b57855ddbd14e3a172c40488b2d5c1a16fc420896316a0df053d734e4cd8e3083fc5cb8a73d3dd3785841629b57855ddbd14e3a16901a16c01a171c41069c41c05952ff04e3a85ab84308b680da16611a16dc4289e5812aaca15c16a87ed653a2ba66a913f7321b838797399b1345d19cf17141453d40e601bfd8856",
    )
    .unwrap();
    let advertisement = Advertisement::parse(&bytes).unwrap();
    assert_eq!(advertisement.transfer_size, 4_400);
    assert_eq!(advertisement.data_size, 4_343);
    assert_eq!(advertisement.parts, 10);
    assert_eq!(advertisement.flags, FLAG_ENCRYPTED | FLAG_RESPONSE);
    assert_eq!(advertisement.hashmap_parts(), 10);
    assert_eq!(
        advertisement.q,
        Some(hex::decode("69c41c05952ff04e3a85ab84308b680d").unwrap())
    );
}

/// Re-packing the parsed advertisement reproduces the exact captured bytes. This is the
/// proof the codec is faithful, key order and all.
#[test]
fn repacks_to_the_exact_captured_bytes() {
    let a = Advertisement::parse(&adv_bytes()).unwrap();
    assert_eq!(a.pack(), adv_bytes());
}

#[test]
fn request_codec_round_trips() {
    let rh = [0x11u8; 32];
    let wanted = [[1u8; 4], [2u8; 4], [3u8; 4]];
    let r = parse_request(&build_request(&rh, &wanted)).unwrap();
    assert!(!r.exhausted);
    assert_eq!(r.last_map_hash, None);
    assert_eq!(r.resource_hash, rh);
    assert_eq!(r.wanted, wanted);

    let last = [9u8; 4];
    let e = parse_request(&build_exhausted_request(&last, &rh, &wanted)).unwrap();
    assert!(e.exhausted);
    assert_eq!(e.last_map_hash, Some(last));
    assert_eq!(e.resource_hash, rh);
    assert_eq!(e.wanted, wanted);
}

#[test]
fn hmu_codec_round_trips() {
    let rh = [0x22u8; 32];
    let hashes = [[0xAu8; 4], [0xBu8; 4], [0xCu8; 4]];
    let h = parse_hmu(&build_hmu(&rh, 1, &hashes)).unwrap();
    assert_eq!(h.resource_hash, rh);
    assert_eq!(h.segment, 1);
    assert_eq!(h.hashes, hashes);
}

/// Integers go out in the shortest form Python's msgpack would use, and any width a peer
/// chose reads back.
#[test]
fn integers_are_shortest_form_and_read_in_any_width() {
    let rh = [0x22u8; 32];
    let hmu = build_hmu(&rh, 200, &[[1; 4]]);
    assert_eq!(hmu[32..35], [0x92, 0xcc, 200]);
    assert_eq!(parse_hmu(&hmu).unwrap().segment, 200);
    for (segment, wide) in [
        (300, vec![0xcd, 0x01, 0x2c]),
        (70_000, vec![0xce, 0, 1, 0x11, 0x70]),
        (5, vec![0xd0, 5]),
        (-40, vec![0xd0, 0xd8]),
        (-300, vec![0xd1, 0xfe, 0xd4]),
    ] {
        let mut hmu = rh.to_vec();
        hmu.push(0x92);
        hmu.extend_from_slice(&wide);
        hmu.extend_from_slice(&[0xc4, 0]);
        assert_eq!(parse_hmu(&hmu).unwrap().segment, segment, "{wide:02x?}");
    }

    let mut a = Advertisement::parse(&adv_bytes()).unwrap();
    a.i = 300;
    a.l = 70_000;
    let packed = a.pack();
    assert!(packed.windows(4).any(|w| w == [0xa1, b'i', 0xcd, 0x01]));
    assert!(packed.windows(4).any(|w| w == [0xa1, b'l', 0xce, 0x00]));
    assert_eq!(Advertisement::parse(&packed).unwrap(), a);
}

/// A part request starts with `0x00` or `0xff`; any other first byte is malformed.
#[test]
fn a_request_with_an_unknown_flag_is_refused() {
    let mut request = build_request(&[0x11; 32], &[[1; 4]]);
    request[0] = 0x01;
    assert_eq!(parse_request(&request), Err(Error::BadRequest));
    assert_eq!(parse_request(&[]), Err(Error::BadRequest));
}

/// A segment of a multi-segment resource advertises the split flag (`Resource.py` 1318).
#[test]
fn a_multi_segment_advertisement_sets_the_split_flag() {
    let single = Outgoing::new(b"data", b"token", [1, 2, 3, 4], false);
    assert_eq!(single.advertisement().flags & FLAG_SPLIT, 0);
    let split = single.with_segment(1, 2, 8, [0; 32]);
    assert_ne!(split.advertisement().flags & FLAG_SPLIT, 0);
}

/// The captured RNS HMU decodes to the known structure.
#[test]
fn hmu_matches_captured_rns() {
    let hmu = hex::decode(
        "34fa88d9f5bbe24374673ed08a7a1748c8cef5a281c5d82866f530026d863a08\
         9201c43456db7769dec46a395226df3ef3d0ac23c4ef932b3b5626381e0ef732\
         1238cf007f73a344c91aa2590e7ec740c3c3ea1fe51bc895",
    )
    .unwrap();
    let h = parse_hmu(&hmu).unwrap();
    assert_eq!(h.segment, 1);
    assert_eq!(h.hashes.len(), 13);
    assert_eq!(
        hex::encode(h.resource_hash),
        "34fa88d9f5bbe24374673ed08a7a1748c8cef5a281c5d82866f530026d863a08"
    );
}

#[test]
fn metadata_frames_and_splits() {
    let framed = pack_metadata(b"meta").unwrap();
    assert_eq!(framed, b"\x00\x00\x04meta");
    let mut data = framed;
    data.extend_from_slice(b"body");
    assert_eq!(split_metadata(&mut data).unwrap(), b"meta");
    assert_eq!(data, b"body");
    let mut short = b"\x00\x00\x09meta".to_vec();
    assert_eq!(split_metadata(&mut short), Err(Error::Truncated));
    assert_eq!(
        pack_metadata(&vec![0; METADATA_MAX_SIZE + 1]),
        Err(Error::CapacityExceeded)
    );
}

#[test]
fn proof_packet_round_trips() {
    let h = [0x11; 32];
    let p = [0x22; 32];
    let mut payload = h.to_vec();
    payload.extend_from_slice(&p);
    assert_eq!(parse_proof(&payload), Some((h, p)));
    assert_eq!(parse_proof(&payload[..63]), None);
}

#[cfg(feature = "compression")]
#[test]
fn compress_round_trips() {
    // Compressible input so bz2 actually shrinks it.
    let content: Vec<u8> = (0..8000u32).map(|i| (i / 40) as u8).collect();
    let squished = compress(&content);
    assert!(squished.len() < content.len());
    assert_eq!(decompress(&squished).unwrap(), content);
}

/// Recovery inflates a compressed body only up to its limit: an exact fit (past the
/// bounded decoder's first allocation, so it grows) is returned, one byte over is a
/// typed refusal rather than the data.
#[cfg(feature = "compression")]
#[test]
fn recovery_bounds_the_decompressed_size() {
    let data: Vec<u8> = (0..20_000u32).map(|i| (i / 100) as u8).collect();
    let random_hash = [0x0A, 0x0B, 0x0C, 0x0D];
    let squished = compress(&data);
    let (adv, _) = advertise(&data, &squished, random_hash, true);
    let incoming = Incoming::new(&adv).unwrap();
    let decrypted = content(&squished, &random_hash);

    assert_eq!(
        incoming.recover_with_limit(&decrypted, data.len()),
        Ok(data.clone())
    );
    assert_eq!(
        incoming.recover_with_limit(&decrypted, data.len() - 1),
        Err(Error::DecompressionLimit)
    );
    assert_eq!(incoming.recover(&decrypted), Ok(data));
}

#[test]
fn hash_map_and_proof_derivations() {
    let data = b"the quick brown fox";
    let rh = [0x11, 0x22, 0x33, 0x44];
    let h = resource_hash(data, &rh);
    // map hash is a prefix of SHA256(part || rh)
    let mh = map_hash(data, &rh);
    assert_eq!(
        &crate::hash::full_hash(&[&data[..], &rh[..]].concat())[..4],
        &mh
    );
    // proof folds the resource hash back in
    assert_eq!(
        proof(data, &h),
        crate::hash::full_hash(&[&data[..], &h[..]].concat())
    );
}
