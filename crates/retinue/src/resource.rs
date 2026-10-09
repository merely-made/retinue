//! Resources: segmented transfer of large payloads over a link.
//!
//! The advertisement codec and the [`Outgoing`]/[`Incoming`] state machines, verified against
//! captured RNS traffic, loss tests, and live stock clients. The advertisement is a msgpack
//! map with single-letter keys, decoded from a real RNS 1.3.8 advertisement:
//!
//! ```text
//! t  transfer size   (bytes on the wire, after compression)
//! d  data size       (uncompressed)
//! n  parts           (number of segments)
//! h  resource hash   (32)
//! o  original hash   (32, of the uncompressed data)
//! r  random hash     (4)
//! f  flags
//! m  hashmap         (MAPHASH_LEN = 4 bytes per part)
//! i  segment index
//! l  total segments
//! q  binary request id for a response Resource, nil otherwise
//! ```

mod advertisement;
#[cfg(feature = "compression")]
mod compression;
mod control;
mod framing;
mod hashes;
mod incoming;
mod map_codec;
mod outgoing;
#[cfg(test)]
mod tests;

pub use advertisement::{Advertisement, advertise};
#[cfg(feature = "compression")]
pub(crate) use compression::{BoundedDecompressError, compress_after, decompress_bounded};
#[cfg(feature = "compression")]
pub use compression::{compress, decompress};
pub use control::{
    Hmu, Request, build_exhausted_request, build_hmu, build_request, parse_hmu, parse_request,
};
pub use framing::{
    content, data_from_content, pack_metadata, split_metadata, split_parts, split_parts_with_size,
};
pub use hashes::{map_hash, parse_proof, proof, resource_hash};
pub use incoming::Incoming;
pub use outgoing::Outgoing;

/// Bytes of a part's map hash in the advertisement hashmap.
pub const MAPHASH_LEN: usize = 4;

/// Length of a random hash.
pub const RANDOM_HASH_LEN: usize = 4;

/// Segment data unit: the maximum size of one part's payload. `RNS.Resource.SDU`.
pub const SDU: usize = 464;

/// Advertisement flag bit: the payload is encrypted (always set, over a link).
pub const FLAG_ENCRYPTED: u64 = 0x01;
/// Advertisement flag bit: the payload is bz2-compressed.
pub const FLAG_COMPRESSED: u64 = 0x02;
/// Advertisement flag bit: the resource is one segment of a larger, split resource.
pub const FLAG_SPLIT: u64 = 0x04;
/// Advertisement flag bit: this Resource carries a request too large for one packet.
pub const FLAG_REQUEST: u64 = 0x08;
/// Advertisement flag bit: this Resource carries a request response.
pub const FLAG_RESPONSE: u64 = 0x10;
/// Advertisement flag bit: the first segment's data starts with metadata (see
/// [`pack_metadata`]). `RNS.ResourceAdvertisement`'s `x` flag.
pub const FLAG_METADATA: u64 = 0x20;

/// The largest packed metadata a resource carries: its length rides in three bytes.
/// `RNS.Resource.METADATA_MAX_SIZE`.
pub const METADATA_MAX_SIZE: usize = (1 << 24) - 1;

/// The most bytes a received compressed resource may decompress to unless a receiver is
/// told otherwise: 64 MiB, RNS's `Resource.AUTO_COMPRESS_MAX_SIZE`, which RNS also uses as
/// its receive-side ceiling.
pub const DEFAULT_MAX_DECOMPRESSED_SIZE: usize = 64 * 1024 * 1024;

/// Bytes of hashmap an advertisement carries at most. `RNS.ResourceAdvertisement`'s
/// `HASHMAP_MAX_LEN` is 74 part-hashes; the rest stream via [`Hmu`].
pub const HASHMAP_MAX_PARTS: usize = 74;

/// The most parts a receiver accepts for one segment unless told otherwise: roughly 1 MB at
/// the default part size, which is the single-segment ceiling the format already implies.
pub const DEFAULT_MAX_PARTS: usize = 4096;

/// The widest receive window: how far past the first missing part a receiver requests and
/// matches parts. `RNS.Resource.WINDOW_MAX_FAST`.
pub const WINDOW_MAX: usize = 75;

/// Bytes of payload per segment. A resource larger than this splits into multiple segments,
/// each transferred (and proved) independently. `RNS.Resource.MAX_EFFICIENT_SIZE`.
pub const MAX_SEGMENT_SIZE: usize = 1_048_575;
