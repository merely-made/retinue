//! bz2 compression of resource content.

use alloc::vec::Vec;

use crate::{Error, Result};

/// Compress content with bz2, as RNS does when compression helps.
///
/// RNS compresses the `random_hash || data` content, and only keeps the compressed form if
/// it is smaller. Returns the bz2 bytes; the caller decides whether to use them (and sets
/// [`FLAG_COMPRESSED`](super::FLAG_COMPRESSED) accordingly) by comparing lengths. Available under the `compression`
/// feature.
pub fn compress(content: &[u8]) -> Vec<u8> {
    compress_after(Vec::new(), content)
}

/// [`compress`] `content` onto the end of `out`, so a prefix needs no second copy.
pub(crate) fn compress_after(out: Vec<u8>, content: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = bzip2::write::BzEncoder::new(out, bzip2::Compression::best());
    enc.write_all(content)
        .expect("writing to a Vec cannot fail");
    enc.finish().expect("finishing a Vec encoder cannot fail")
}

/// Decompress bz2 content. The inverse of [`compress`]. Returns [`Error::BadPadding`] on
/// malformed input.
///
/// Unbounded: the output grows to whatever the input inflates to. A received resource is
/// recovered through [`Incoming::recover_with_limit`](super::Incoming::recover_with_limit) instead, which bounds it.
pub fn decompress(compressed: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut dec = bzip2::read::BzDecoder::new(compressed);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).map_err(|_| Error::BadPadding)?;
    Ok(out)
}

/// Failure of the bounded bz2 decoder used by resource recovery and stream frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundedDecompressError {
    InvalidData,
    LimitExceeded,
}

/// Decode bz2 with a hard bound on the owned output `Vec` allocation.
///
/// The output grows geometrically as data actually arrives, and never past `limit + 1`
/// bytes: the extra sentinel byte distinguishes an exact fit from an oversized input. A
/// large limit therefore costs nothing up front, and an oversized input is refused once it
/// has produced `limit + 1` bytes. The bz2 decoder's own workspace is separate and is not
/// covered by this output bound.
pub(crate) fn decompress_bounded(
    compressed: &[u8],
    limit: usize,
) -> core::result::Result<Vec<u8>, BoundedDecompressError> {
    use std::io::Read;

    /// First output allocation; doubled as the decoder fills it.
    const INITIAL: usize = 4096;

    // Callers validate the limit before storing it; never allow an overflowing or
    // unrepresentable allocation even if this helper is called directly.
    let capacity = limit
        .checked_add(1)
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or(BoundedDecompressError::LimitExceeded)?;
    let mut output = Vec::new();
    let mut decoder = bzip2::read::BzDecoder::new(compressed);
    let mut used = 0;
    loop {
        if used == output.len() {
            if used == capacity {
                return Err(BoundedDecompressError::LimitExceeded);
            }
            let grown = used
                .saturating_mul(2)
                .clamp(INITIAL.min(capacity), capacity);
            output.reserve_exact(grown - used);
            output.resize(grown, 0);
        }
        let n = decoder
            .read(&mut output[used..])
            .map_err(|_| BoundedDecompressError::InvalidData)?;
        if n == 0 {
            output.truncate(used);
            return Ok(output);
        }
        used += n;
    }
}
