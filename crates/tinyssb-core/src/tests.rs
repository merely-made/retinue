extern crate std;

use core::fmt::Write;

use sha2::{Digest, Sha256};

use super::*;
use crate::verify::{decode_varint, frame_hash, sidechain_requirement};

fn feed() -> FeedId {
    FeedId::new([
        0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e, 0xab,
        0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06, 0x98, 0x81,
        0xdb, 0x12,
    ])
}

fn fixture<const N: usize>(encoded: &str) -> [u8; N] {
    let bytes = encoded.as_bytes();
    assert_eq!(
        bytes.len(),
        N * 2 + 1,
        "fixture must be one hex line plus newline"
    );
    let mut decoded = [0; N];
    for (index, destination) in decoded.iter_mut().enumerate() {
        *destination = hex_nibble(bytes[index * 2]) << 4 | hex_nibble(bytes[index * 2 + 1]);
    }
    decoded
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => panic!("fixture contains non-lowercase-hex byte"),
    }
}

fn fixture_sha256(bytes: &[u8; FRAME_LEN]) -> std::string::String {
    let mut rendered = std::string::String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(rendered, "{byte:02x}").unwrap();
    }
    rendered
}

#[test]
fn fixture_manifest_covers_every_frame_and_its_claims() {
    let manifest = include_str!("../fixtures/manifest.toml");
    assert!(manifest.contains("https://github.com/ssbc/tinySSB"));
    assert!(manifest.contains("39896b72c97b51159d46610c5f11ff7f5a279031"));
    assert!(manifest.contains("upstream_license = \"MIT\""));
    assert!(manifest.contains("esp32/loramesh-TBeam/replica.cpp"));
    assert!(manifest.contains("Codec2"));

    let fixtures = [
        (
            "plain-1.hex",
            include_str!("../fixtures/plain-1.hex"),
            "825432f927288ca891f8764e317cae1b8feda2a68466cb9c76b4b1bd61a568d4",
        ),
        (
            "sidechain-1.hex",
            include_str!("../fixtures/sidechain-1.hex"),
            "8e95d12a35b9c1f9071f42eac7c6bf2381fc356bc94dfcfb8bf07534e18cb75a",
        ),
        (
            "sidechain-1.chunk-1.hex",
            include_str!("../fixtures/sidechain-1.chunk-1.hex"),
            "4b13406e5d984d2ae49d6f02a1cc272837c68eabb0f3c87e2855f83cde6c1eb9",
        ),
        (
            "sidechain-1.chunk-2.hex",
            include_str!("../fixtures/sidechain-1.chunk-2.hex"),
            "b51752f85c5264000e851b7e2651a4d5393c27d8e63ebf6e72be424244aa1692",
        ),
        (
            "plain-1.bad-dmx.hex",
            include_str!("../fixtures/plain-1.bad-dmx.hex"),
            "d8f54c5599df203c3b7365af375f6ae472f38d69ddc2a3b490ce48b377946f18",
        ),
        (
            "plain-1.bad-signature.hex",
            include_str!("../fixtures/plain-1.bad-signature.hex"),
            "7762bdcfe876aa62fc3f064a837a39d90e12e1278cd56c8f6199a3496d6882c0",
        ),
        (
            "sidechain-1.chunk-1.bad-pointer.hex",
            include_str!("../fixtures/sidechain-1.chunk-1.bad-pointer.hex"),
            "7ca2844460a5234c256589663b327484f189c693f16889410c98913f981d6ffd",
        ),
    ];

    let fixture_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let disk_hex_count = std::fs::read_dir(fixture_directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "hex")
        })
        .count();
    assert_eq!(disk_hex_count, fixtures.len());
    assert_eq!(manifest.matches("[[fixture]]").count(), fixtures.len());

    for (path, encoded, expected_sha256) in fixtures {
        let frame = fixture::<FRAME_LEN>(encoded);
        assert_eq!(
            fixture_sha256(&frame),
            expected_sha256,
            "fixture checksum: {path}"
        );
        assert!(manifest.contains(path), "manifest path: {path}");
        assert!(
            manifest.contains(expected_sha256),
            "manifest checksum: {path}"
        );
    }
}

#[test]
fn plain_next_entry_verifies_and_advances_frontier() {
    let feed = feed();
    let frontier = Frontier::initial(feed);
    let frame = MainFrame::new(fixture(include_str!("../fixtures/plain-1.hex")));

    let verified = verify_next(feed, frontier, &frame, 0).unwrap();
    assert_eq!(verified.sequence(), 1);
    assert_eq!(verified.next_frontier().sequence(), 1);
    assert_ne!(verified.message_id(), frontier.previous());
    let expected: [u8; PLAIN_CONTENT_LEN] = core::array::from_fn(|index| index as u8);
    assert_eq!(verified.content(), EntryContent::Plain(&expected));
}

#[test]
fn initial_frontier_and_wrong_continuity_are_refused() {
    let feed = feed();
    let initial = Frontier::initial(feed);
    assert_eq!(
        initial.previous(),
        MessageId::new(feed.as_bytes()[..MESSAGE_ID_LEN].try_into().unwrap())
    );

    let frame = MainFrame::new(fixture(include_str!("../fixtures/plain-1.hex")));
    let wrong_predecessor = Frontier::from_verified(0, MessageId::ZERO);
    assert_eq!(
        verify_next(feed, wrong_predecessor, &frame, 0),
        Err(Refusal::DmxMismatch)
    );

    let verified = verify_next(feed, initial, &frame, 0).unwrap();
    let wrong_sequence = Frontier::from_verified(1, verified.message_id());
    assert_eq!(
        verify_next(feed, wrong_sequence, &frame, 0),
        Err(Refusal::DmxMismatch)
    );
}

#[test]
fn sidechain_verifies_in_exact_hash_order() {
    let feed = feed();
    let frontier = Frontier::initial(feed);
    let main = MainFrame::new(fixture(include_str!("../fixtures/sidechain-1.hex")));
    let first = ChunkFrame::new(fixture(include_str!("../fixtures/sidechain-1.chunk-1.hex")));
    let second = ChunkFrame::new(fixture(include_str!("../fixtures/sidechain-1.chunk-2.hex")));
    let verified = verify_next(feed, frontier, &main, 2).unwrap();
    let EntryContent::Sidechain(requirement) = verified.content() else {
        panic!("must be a side chain");
    };
    assert_eq!(requirement.declared_len(), 128);
    assert_eq!(requirement.cursor().remaining_chunks(), 2);
    let ChunkProgress::Next(cursor) = verify_chunk(requirement.cursor(), &first).unwrap() else {
        panic!("first chunk must lead to second");
    };
    assert_eq!(cursor.remaining_chunks(), 1);
    assert_eq!(verify_chunk(cursor, &second), Ok(ChunkProgress::Complete));
}

#[test]
fn malformed_and_over_capacity_frames_are_refused() {
    let feed = feed();
    let frontier = Frontier::initial(feed);
    let bad_dmx = MainFrame::new(fixture(include_str!("../fixtures/plain-1.bad-dmx.hex")));
    assert_eq!(
        verify_next(feed, frontier, &bad_dmx, 0),
        Err(Refusal::DmxMismatch)
    );

    let bad_signature = MainFrame::new(fixture(include_str!(
        "../fixtures/plain-1.bad-signature.hex"
    )));
    assert_eq!(
        verify_next(feed, frontier, &bad_signature, 0),
        Err(Refusal::BadSignature)
    );

    let chain = MainFrame::new(fixture(include_str!("../fixtures/sidechain-1.hex")));
    assert_eq!(
        verify_next(feed, frontier, &chain, 1),
        Err(Refusal::ChunkCapacityExceeded {
            required: 2,
            maximum: 1,
        })
    );

    let mut malformed_length = *chain.as_bytes();
    malformed_length[CONTENT_OFFSET..SIDECHAIN_POINTER_OFFSET].fill(0x80);
    assert_eq!(
        sidechain_requirement(&malformed_length, 2),
        Err(Refusal::DeclaredLengthOverflow)
    );
    assert_eq!(decode_varint(&[0x80; 8]), Err(Refusal::MalformedLength));

    let mut undersized_length = *chain.as_bytes();
    undersized_length[CONTENT_OFFSET] = 0;
    assert_eq!(
        sidechain_requirement(&undersized_length, 2),
        Err(Refusal::DeclaredLengthTooSmall)
    );

    let corrupt_chunk = ChunkFrame::new(fixture(include_str!(
        "../fixtures/sidechain-1.chunk-1.bad-pointer.hex"
    )));
    let verified = verify_next(feed, frontier, &chain, 2).unwrap();
    let EntryContent::Sidechain(requirement) = verified.content() else {
        panic!("must be a side chain");
    };
    assert_eq!(
        verify_chunk(requirement.cursor(), &corrupt_chunk),
        Err(Refusal::ChunkHashMismatch)
    );

    let forced_cursor = ChunkCursor::from_verified(frame_hash(corrupt_chunk.as_bytes()), 2);
    let ChunkProgress::Next(next) = verify_chunk(forced_cursor, &corrupt_chunk).unwrap() else {
        panic!("two chunk cursor must advance");
    };
    let second = ChunkFrame::new(fixture(include_str!("../fixtures/sidechain-1.chunk-2.hex")));
    assert_eq!(verify_chunk(next, &second), Err(Refusal::ChunkHashMismatch));
}
