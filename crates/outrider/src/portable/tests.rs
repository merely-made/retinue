use super::msgpack::{MAX_NESTING, skip};
use super::*;
use alloc::vec;

#[derive(serde::Deserialize)]
struct OracleCapture {
    destination: String,
    source: String,
    message_id: String,
    signature: String,
    packed: String,
}

fn array<const N: usize>(hex_value: &str) -> [u8; N] {
    hex::decode(hex_value).unwrap().try_into().unwrap()
}

fn oracle() -> OracleCapture {
    serde_json::from_str(include_str!("../../tests/fixtures/lxmf_message.json")).unwrap()
}

/// The stock LXMF 0.9.6 capture decodes to the same facts the shipping codec reads,
/// including the message id and signing preimage.
#[cfg(feature = "std")]
#[test]
fn the_oracle_capture_decodes_to_the_same_facts() {
    let oracle = oracle();
    let packed = hex::decode(&oracle.packed).unwrap();
    let mine = decode(&packed).unwrap();
    let theirs = crate::codec::decode(&packed).unwrap();

    assert_eq!(mine.destination, array(&oracle.destination));
    assert_eq!(mine.source, array(&oracle.source));
    assert_eq!(mine.signature, array(&oracle.signature));
    assert_eq!(mine.message_id, array::<32>(&oracle.message_id));
    assert_eq!(mine.payload.timestamp, 1_753_603_200.5);
    assert_eq!(mine.payload.title, b"TITLE");
    assert_eq!(mine.payload.content, b"BODY");
    // The fields map is carried as the bytes it arrived as: a map holding key 7.
    assert_eq!(
        mine.payload.fields,
        vec![0x81, 0x07, 0xc4, 0x04, b'm', b'e', b't', b'a']
    );

    assert_eq!(mine.message_id, theirs.message_id);
    assert_eq!(mine.signing_bytes(), theirs.signing_bytes());
}

/// Re-encoding reproduces the captured bytes exactly.
#[test]
fn re_encoding_the_oracle_payload_reproduces_its_exact_bytes() {
    let oracle = oracle();
    let packed = hex::decode(&oracle.packed).unwrap();
    let decoded = decode(&packed).unwrap();

    let re_encoded = encode_payload(&decoded.payload, false).unwrap();
    assert_eq!(
        re_encoded,
        &packed[HEADER_LEN..],
        "a decoded payload must re-encode to the bytes it came from",
    );
}

/// Both codecs agree on lengths across MessagePack's bin8/bin16 boundary, where the
/// binary encoding changes.
#[cfg(feature = "std")]
#[test]
fn the_two_codecs_agree_across_the_binary_length_boundary() {
    for len in [0_usize, 1, 31, 254, 255, 256, 257, 1000] {
        let content = vec![0xab_u8; len];
        let payload = crate::codec::LxmfPayload::text(1_753_603_200.5, b"t", content.clone());
        let prepared = crate::codec::prepare([9; 16], [8; 16], &payload).unwrap();
        let packed = prepared.finish([7; 64]);

        let mine = decode(&packed).unwrap();
        let theirs = crate::codec::decode(&packed).unwrap();
        assert_eq!(mine.message_id, theirs.message_id, "len {len}");
        assert_eq!(mine.signing_bytes(), theirs.signing_bytes(), "len {len}");
        assert_eq!(mine.payload.content, content, "len {len}");
        assert_eq!(
            encode_payload(&mine.payload, false).unwrap(),
            &packed[HEADER_LEN..],
            "len {len}: re-encode must be byte-exact",
        );
    }
}

/// A stamp does not change the message id; the unstamped re-encode exists for this.
#[test]
fn a_stamp_does_not_change_the_message_id() {
    let mut payload = Payload::text(1_753_603_200.5, b"title", b"body");
    let unstamped = encode_payload(&payload, false).unwrap();
    let id = message_id([1; 16], [2; 16], &unstamped);

    payload.stamp = Some(vec![3; 16]);
    let stamped = encode_payload(&payload, true).unwrap();
    let mut packed = Vec::new();
    packed.extend_from_slice(&[1_u8; 16]);
    packed.extend_from_slice(&[2_u8; 16]);
    packed.extend_from_slice(&[4_u8; 64]);
    packed.extend_from_slice(&stamped);

    let decoded = decode(&packed).unwrap();
    assert_eq!(decoded.message_id, id);
    assert_eq!(decoded.payload.stamp, Some(vec![3; 16]));
}

/// The skipper decides where `fields` ends, so it is swept over every container and width
/// a field map may legally hold.
#[test]
fn the_skipper_measures_every_value_shape() {
    let cases: Vec<Vec<u8>> = vec![
        vec![0xc0],                                     // nil
        vec![0xc2],                                     // false
        vec![0x07],                                     // positive fixint
        vec![0xff],                                     // negative fixint
        vec![0xcc, 0x80],                               // uint8
        vec![0xcd, 0x01, 0x02],                         // uint16
        vec![0xce, 1, 2, 3, 4],                         // uint32
        vec![0xcf, 1, 2, 3, 4, 5, 6, 7, 8],             // uint64
        vec![0xca, 1, 2, 3, 4],                         // float32
        vec![0xcb, 1, 2, 3, 4, 5, 6, 7, 8],             // float64
        vec![0xa3, b'a', b'b', b'c'],                   // fixstr
        vec![0xd9, 2, b'h', b'i'],                      // str8
        vec![0xc4, 2, 1, 2],                            // bin8
        vec![0xc5, 0, 2, 1, 2],                         // bin16
        vec![0x80],                                     // empty map
        vec![0x81, 0x07, 0xc4, 0x01, 0x09],             // fixmap with a bin value
        vec![0x92, 0x01, 0x02],                         // array of two
        vec![0x81, 0x01, 0x92, 0x01, 0x81, 0x02, 0x03], // nesting
        vec![0xd4, 0x00, 0x01],                         // fixext1
        vec![0xc7, 0x02, 0x00, 0x01, 0x02],             // ext8
    ];
    for case in cases {
        let mut at = 0;
        skip(&case, &mut at).unwrap_or_else(|error| panic!("{case:02x?}: {error}"));
        assert_eq!(at, case.len(), "{case:02x?} was mis-measured");
    }

    // And a value that runs off the end is refused rather than measured optimistically.
    let mut at = 0;
    assert!(skip(&[0xc4, 40, 1, 2], &mut at).is_err());
}

/// Untrusted nesting costs a stack frame per byte of `0x91`; past the limit it is refused.
#[test]
fn deep_nesting_is_refused_rather_than_recursed() {
    // Well inside the limit: still read normally.
    let mut shallow = alloc::vec![0x91_u8; MAX_NESTING as usize - 2];
    shallow.push(0xc0); // nil, to terminate the innermost array
    let mut at = 0;
    assert!(
        skip(&shallow, &mut at).is_ok(),
        "ordinary nesting still parses"
    );

    // Past it: refused, and the refusal is a value rather than a crash.
    let mut deep = alloc::vec![0x91_u8; 400];
    deep.push(0xc0);
    let mut at = 0;
    assert_eq!(
        skip(&deep, &mut at),
        Err(CodecError::MalformedMessagePack),
        "a nest deeper than the limit is refused, not followed",
    );
}

/// A map count is doubled before reading, which wraps on 32-bit boards, so the multiply is
/// checked. A 64-bit host cannot witness the wrap; this holds that the count is refused.
#[test]
fn an_absurd_map_count_is_refused() {
    let mut bytes = alloc::vec![0xdf_u8];
    bytes.extend_from_slice(&u32::MAX.to_be_bytes());
    let mut at = 0;
    assert_eq!(
        skip(&bytes, &mut at),
        Err(CodecError::MalformedMessagePack),
        "four billion entries in five bytes is refused, not attempted",
    );
}

#[test]
fn malformed_messages_are_refused() {
    assert_eq!(decode(&[0; 95]), Err(CodecError::TruncatedHeader));
    let oracle = oracle();
    let mut trailing = hex::decode(&oracle.packed).unwrap();
    trailing.push(0);
    assert_eq!(decode(&trailing), Err(CodecError::MalformedMessagePack));
    // Fields must be a map: the one thing this codec asserts about them.
    let mut payload = Payload::text(1.0, b"t", b"b");
    payload.fields = vec![0x92, 0x01, 0x02];
    assert_eq!(
        encode_payload(&payload, false),
        Err(CodecError::InvalidFields)
    );
}
