//! The wire types and the caller-owned verification state.

use crate::{FEED_ID_LEN, FRAME_LEN, MESSAGE_ID_LEN};

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
    pub(crate) sequence: u32,
    pub(crate) previous: MessageId,
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
/// maximum chunk count supplied to [`verify_next`](crate::verify_next).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SidechainRequirement<'a> {
    pub(crate) declared_len: u32,
    pub(crate) inline_content: &'a [u8],
    pub(crate) cursor: ChunkCursor,
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
    pub(crate) expected_hash: MessageId,
    pub(crate) remaining_chunks: u32,
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
    pub(crate) sequence: u32,
    pub(crate) message_id: MessageId,
    pub(crate) next_frontier: Frontier,
    pub(crate) content: EntryContent<'a>,
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
