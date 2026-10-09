//! Main-entry and side-chain verification.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::{
    CHUNK_CONTENT_LEN, CONTENT_OFFSET, ChunkCursor, ChunkFrame, ChunkProgress, DMX_LEN, DOMAIN,
    EntryContent, FEED_ID_LEN, FRAME_LEN, FeedId, Frontier, MESSAGE_ID_LEN, MainFrame, MessageId,
    Refusal, SIDECHAIN_POINTER_OFFSET, SIGNATURE_OFFSET, SIGNED_INPUT_LEN, SIGNED_MAIN_LEN,
    SidechainRequirement, TYPE_OFFSET, VerifiedEntry,
};

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

pub(crate) fn sidechain_requirement<'a>(
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

pub(crate) fn decode_varint(bytes: &[u8]) -> Result<(u64, usize), Refusal> {
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

pub(crate) fn frame_hash(frame: &[u8; FRAME_LEN]) -> MessageId {
    truncate_hash(Sha256::digest(frame))
}

fn truncate_hash(digest: impl AsRef<[u8]>) -> MessageId {
    let mut id = [0; MESSAGE_ID_LEN];
    id.copy_from_slice(&digest.as_ref()[..MESSAGE_ID_LEN]);
    MessageId::new(id)
}
