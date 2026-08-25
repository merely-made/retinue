#![no_std]
#![forbid(unsafe_code)]

//! Exact verification for the pinned tinySSB v0 signed-feed wire format.
//!
//! This crate verifies a single next main entry for caller-owned feed state and
//! advances a caller-owned side-chain cursor. It deliberately has no radio,
//! retry, persistence, clock, entropy, feed-table, or allocator dependency.
//! The wire rules are independently implemented from tinySSB's MIT ESP32 core
//! at revision `39896b72c97b51159d46610c5f11ff7f5a279031`; see `NOTICE` and
//! `fixtures/manifest.toml`.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// The domain prepended to every tinySSB v0 main-entry hash and signature.
pub const DOMAIN: &[u8; 10] = b"tinyssb-v0";
/// Bytes in every tinySSB main entry and side-chain chunk.
pub const FRAME_LEN: usize = 120;
/// Bytes in an Ed25519 feed public key.
pub const FEED_ID_LEN: usize = 32;
/// Bytes in the truncated SHA-256 message and chunk identifiers.
pub const MESSAGE_ID_LEN: usize = 20;
/// Bytes in a derived DMX header.
pub const DMX_LEN: usize = 7;
/// Bytes of the signed main-entry prefix before its signature.
pub const SIGNED_MAIN_LEN: usize = 56;
/// Bytes of plain inline content.
pub const PLAIN_CONTENT_LEN: usize = 48;
/// Bytes in a side-chain chunk before its successor hash.
pub const CHUNK_CONTENT_LEN: usize = 100;

const TYPE_OFFSET: usize = DMX_LEN;
const CONTENT_OFFSET: usize = TYPE_OFFSET + 1;
const SIGNATURE_OFFSET: usize = SIGNED_MAIN_LEN;
const SIDECHAIN_POINTER_OFFSET: usize = CONTENT_OFFSET + PLAIN_CONTENT_LEN - MESSAGE_ID_LEN;
const SIGNED_INPUT_LEN: usize = DOMAIN.len() + FEED_ID_LEN + 4 + MESSAGE_ID_LEN + SIGNED_MAIN_LEN;

/// A tinySSB feed's Ed25519 public key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct FeedId([u8; FEED_ID_LEN]);

impl FeedId {
    pub const fn new(bytes: [u8; FEED_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; FEED_ID_LEN] {
        &self.0
    }
}

/// A tinySSB message or side-chain hash pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MessageId([u8; MESSAGE_ID_LEN]);

impl MessageId {
    pub const ZERO: Self = Self([0; MESSAGE_ID_LEN]);

    pub const fn new(bytes: [u8; MESSAGE_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; MESSAGE_ID_LEN] {
        &self.0
    }
}

/// One exact 120-byte tinySSB main-chain packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MainFrame([u8; FRAME_LEN]);

impl MainFrame {
    pub const fn new(bytes: [u8; FRAME_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; FRAME_LEN] {
        &self.0
    }
}

/// One exact 120-byte tinySSB side-chain packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkFrame([u8; FRAME_LEN]);

impl ChunkFrame {
    pub const fn new(bytes: [u8; FRAME_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; FRAME_LEN] {
        &self.0
    }
}

/// The caller-owned, verified tip of one feed.
///
/// tinySSB's pinned ESP32 implementation initializes a new feed's previous
/// hash with the first twenty bytes of its feed id. That is intentionally not
/// a zero hash and is part of this exact v0 compatibility surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frontier {
    sequence: u32,
    previous: MessageId,
}

impl Frontier {
    /// The frontier before sequence one.
    pub const fn initial(feed: FeedId) -> Self {
        let id = feed.0;
        Self {
            sequence: 0,
            previous: MessageId([
                id[0], id[1], id[2], id[3], id[4], id[5], id[6], id[7], id[8], id[9], id[10],
                id[11], id[12], id[13], id[14], id[15], id[16], id[17], id[18], id[19],
            ]),
        }
    }

    /// Restore a previously verified feed tip from caller-owned persistence.
    ///
    /// The constructor performs no verification; callers must only persist a
    /// frontier returned by [`VerifiedEntry::next_frontier`].
    pub const fn from_verified(sequence: u32, previous: MessageId) -> Self {
        Self { sequence, previous }
    }

    pub const fn sequence(&self) -> u32 {
        self.sequence
    }

    pub const fn previous(&self) -> MessageId {
        self.previous
    }
}

/// A verified side-chain request. Its state is caller-owned and bounded by the
/// maximum chunk count supplied to [`verify_next`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SidechainRequirement<'a> {
    declared_len: u32,
    inline_content: &'a [u8],
    cursor: ChunkCursor,
}

impl<'a> SidechainRequirement<'a> {
    pub const fn declared_len(&self) -> u32 {
        self.declared_len
    }

    pub const fn inline_content(&self) -> &'a [u8] {
        self.inline_content
    }

    pub const fn cursor(&self) -> ChunkCursor {
        self.cursor
    }
}

/// The expected hash and remaining count for the next side-chain chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkCursor {
    expected_hash: MessageId,
    remaining_chunks: u32,
}

impl ChunkCursor {
    /// Restore a caller-owned side-chain cursor.
    ///
    /// As with [`Frontier::from_verified`], this must only restore state
    /// returned by a previously verified requirement or chunk.
    pub const fn from_verified(expected_hash: MessageId, remaining_chunks: u32) -> Self {
        Self {
            expected_hash,
            remaining_chunks,
        }
    }

    pub const fn expected_hash(&self) -> MessageId {
        self.expected_hash
    }

    pub const fn remaining_chunks(&self) -> u32 {
        self.remaining_chunks
    }
}

/// The verified content shape of a main entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryContent<'a> {
    Plain(&'a [u8]),
    Sidechain(SidechainRequirement<'a>),
}

/// The result of accepting exactly one next main entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedEntry<'a> {
    sequence: u32,
    message_id: MessageId,
    next_frontier: Frontier,
    content: EntryContent<'a>,
}

impl<'a> VerifiedEntry<'a> {
    pub const fn sequence(&self) -> u32 {
        self.sequence
    }

    pub const fn message_id(&self) -> MessageId {
        self.message_id
    }

    pub const fn next_frontier(&self) -> Frontier {
        self.next_frontier
    }

    pub const fn content(&self) -> EntryContent<'a> {
        self.content
    }
}

/// The result of verifying one expected side-chain chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkProgress {
    Next(ChunkCursor),
    Complete,
}

/// Why a frame or cursor was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    SequenceExhausted,
    InvalidFeedId,
    DmxMismatch,
    BadSignature,
    UnknownEntryType(u8),
    MalformedLength,
    DeclaredLengthOverflow,
    DeclaredLengthTooSmall,
    ChunkCapacityExceeded { required: u32, maximum: u32 },
    UnexpectedChunk,
    ChunkHashMismatch,
}

/// Derive the exact next DMX value for one feed frontier.
pub fn expected_dmx(feed: FeedId, frontier: Frontier) -> Result<[u8; DMX_LEN], Refusal> {
    let sequence = frontier
        .sequence
        .checked_add(1)
        .ok_or(Refusal::SequenceExhausted)?;
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    hash.update(feed.as_bytes());
    hash.update(sequence.to_be_bytes());
    hash.update(frontier.previous.as_bytes());
    let digest = hash.finalize();
    let mut dmx = [0; DMX_LEN];
    dmx.copy_from_slice(&digest[..DMX_LEN]);
    Ok(dmx)
}

/// Verify only the next main entry for `frontier`.
///
/// `maximum_chunks` is a caller policy bound. The core does not retain a
/// side-chain table; it returns a cursor that the caller can persist, discard,
/// or advance with [`verify_chunk`].
pub fn verify_next<'a>(
    feed: FeedId,
    frontier: Frontier,
    frame: &'a MainFrame,
    maximum_chunks: u32,
) -> Result<VerifiedEntry<'a>, Refusal> {
    let sequence = frontier
        .sequence
        .checked_add(1)
        .ok_or(Refusal::SequenceExhausted)?;
    let bytes = frame.as_bytes();
    if bytes[..DMX_LEN] != expected_dmx(feed, frontier)? {
        return Err(Refusal::DmxMismatch);
    }

    let verifying_key =
        VerifyingKey::from_bytes(feed.as_bytes()).map_err(|_| Refusal::InvalidFeedId)?;
    let mut signed = [0; SIGNED_INPUT_LEN];
    let mut at = 0;
    signed[at..at + DOMAIN.len()].copy_from_slice(DOMAIN);
    at += DOMAIN.len();
    signed[at..at + FEED_ID_LEN].copy_from_slice(feed.as_bytes());
    at += FEED_ID_LEN;
    signed[at..at + 4].copy_from_slice(&sequence.to_be_bytes());
    at += 4;
    signed[at..at + MESSAGE_ID_LEN].copy_from_slice(frontier.previous.as_bytes());
    at += MESSAGE_ID_LEN;
    signed[at..].copy_from_slice(&bytes[..SIGNED_MAIN_LEN]);
    let signature = Signature::from_bytes(
        &bytes[SIGNATURE_OFFSET..]
            .try_into()
            .expect("signature length"),
    );
    if verifying_key.verify_strict(&signed, &signature).is_err() {
        return Err(Refusal::BadSignature);
    }

    let message_id = message_id(feed, sequence, frontier.previous, frame);
    let next_frontier = Frontier {
        sequence,
        previous: message_id,
    };
    let content = match bytes[TYPE_OFFSET] {
        0 => EntryContent::Plain(&bytes[CONTENT_OFFSET..SIGNED_MAIN_LEN]),
        1 => EntryContent::Sidechain(sidechain_requirement(bytes, maximum_chunks)?),
        unknown => return Err(Refusal::UnknownEntryType(unknown)),
    };

    Ok(VerifiedEntry {
        sequence,
        message_id,
        next_frontier,
        content,
    })
}

/// Verify the next chunk named by a side-chain cursor.
pub fn verify_chunk(cursor: ChunkCursor, frame: &ChunkFrame) -> Result<ChunkProgress, Refusal> {
    if cursor.remaining_chunks == 0 {
        return Err(Refusal::UnexpectedChunk);
    }
    if frame_hash(frame.as_bytes()) != cursor.expected_hash {
        return Err(Refusal::ChunkHashMismatch);
    }
    if cursor.remaining_chunks == 1 {
        return Ok(ChunkProgress::Complete);
    }

    let bytes = frame.as_bytes();
    let mut successor = [0; MESSAGE_ID_LEN];
    successor.copy_from_slice(&bytes[CHUNK_CONTENT_LEN..]);
    Ok(ChunkProgress::Next(ChunkCursor {
        expected_hash: MessageId::new(successor),
        remaining_chunks: cursor.remaining_chunks - 1,
    }))
}

fn sidechain_requirement<'a>(
    bytes: &'a [u8; FRAME_LEN],
    maximum_chunks: u32,
) -> Result<SidechainRequirement<'a>, Refusal> {
    let (declared_len, encoded_len) =
        decode_varint(&bytes[CONTENT_OFFSET..SIDECHAIN_POINTER_OFFSET])?;
    let inline_len = (SIDECHAIN_POINTER_OFFSET - CONTENT_OFFSET)
        .checked_sub(encoded_len)
        .ok_or(Refusal::MalformedLength)?;
    let declared_len = u32::try_from(declared_len).map_err(|_| Refusal::DeclaredLengthOverflow)?;
    if declared_len <= inline_len as u32 {
        return Err(Refusal::DeclaredLengthTooSmall);
    }
    let remaining_bytes = declared_len - inline_len as u32;
    let required_chunks = remaining_bytes.div_ceil(CHUNK_CONTENT_LEN as u32);
    if required_chunks > maximum_chunks {
        return Err(Refusal::ChunkCapacityExceeded {
            required: required_chunks,
            maximum: maximum_chunks,
        });
    }
    let mut pointer = [0; MESSAGE_ID_LEN];
    pointer.copy_from_slice(&bytes[SIDECHAIN_POINTER_OFFSET..SIGNED_MAIN_LEN]);
    Ok(SidechainRequirement {
        declared_len,
        inline_content: &bytes[CONTENT_OFFSET + encoded_len..SIDECHAIN_POINTER_OFFSET],
        cursor: ChunkCursor {
            expected_hash: MessageId::new(pointer),
            remaining_chunks: required_chunks,
        },
    })
}

fn decode_varint(bytes: &[u8]) -> Result<(u64, usize), Refusal> {
    let mut value = 0_u64;
    for (index, byte) in bytes.iter().copied().enumerate() {
        let shift = index.checked_mul(7).ok_or(Refusal::MalformedLength)?;
        if shift >= 64 {
            return Err(Refusal::DeclaredLengthOverflow);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(Refusal::MalformedLength)
}

fn message_id(feed: FeedId, sequence: u32, previous: MessageId, frame: &MainFrame) -> MessageId {
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    hash.update(feed.as_bytes());
    hash.update(sequence.to_be_bytes());
    hash.update(previous.as_bytes());
    hash.update(frame.as_bytes());
    truncate_hash(hash.finalize())
}

fn frame_hash(frame: &[u8; FRAME_LEN]) -> MessageId {
    truncate_hash(Sha256::digest(frame))
}

fn truncate_hash(digest: impl AsRef<[u8]>) -> MessageId {
    let mut id = [0; MESSAGE_ID_LEN];
    id.copy_from_slice(&digest.as_ref()[..MESSAGE_ID_LEN]);
    MessageId::new(id)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::fmt::Write;

    use super::*;

    fn feed() -> FeedId {
        FeedId::new([
            0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e,
            0xab, 0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06,
            0x98, 0x81, 0xdb, 0x12,
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
        let ChunkProgress::Next(cursor) = verify_chunk(requirement.cursor(), &first).unwrap()
        else {
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
}
