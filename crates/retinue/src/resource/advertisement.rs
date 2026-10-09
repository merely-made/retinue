//! The resource advertisement and its msgpack codec.

use alloc::vec::Vec;

use super::map_codec::{MapReader, MapWriter};
use super::{
    FLAG_COMPRESSED, FLAG_ENCRYPTED, FLAG_METADATA, MAPHASH_LEN, RANDOM_HASH_LEN, resource_hash,
    split_parts,
};
use crate::{Error, Result};

/// Build an advertisement for a transfer.
///
/// `token` is the sealed (and possibly compressed) transfer blob; `data` is the original
/// uncompressed payload. `compressed` sets the compression flag. This computes the hashes,
/// splits the token into parts, and returns the advertisement plus the parts to send.
pub fn advertise(
    data: &[u8],
    token: &[u8],
    random_hash: [u8; RANDOM_HASH_LEN],
    compressed: bool,
) -> (Advertisement, Vec<Vec<u8>>) {
    let hash = resource_hash(data, &random_hash);
    let (parts, hashmap) = split_parts(token, &random_hash);
    let mut flags = FLAG_ENCRYPTED;
    if compressed {
        flags |= FLAG_COMPRESSED;
    }
    let adv = Advertisement {
        transfer_size: token.len() as u64,
        data_size: data.len() as u64,
        parts: parts.len() as u64,
        resource_hash: hash.to_vec(),
        original_hash: hash.to_vec(),
        random_hash: random_hash.to_vec(),
        flags,
        hashmap,
        i: 1,
        l: 1,
        q: None,
    };
    (adv, parts)
}

/// A resource transfer advertisement.
///
/// Fields that retinue does not yet interpret (`i`, `l`, `q`) are preserved so an
/// advertisement round-trips exactly, which keeps hashing and signatures over it stable.
#[derive(Clone, Debug, PartialEq)]
pub struct Advertisement {
    /// `t`: size on the wire after compression.
    pub transfer_size: u64,
    /// `d`: uncompressed data size.
    pub data_size: u64,
    /// `n`: number of parts.
    pub parts: u64,
    /// `h`: the resource hash.
    pub resource_hash: Vec<u8>,
    /// `o`: the hash of the uncompressed data.
    pub original_hash: Vec<u8>,
    /// `r`: a random hash for uniqueness.
    pub random_hash: Vec<u8>,
    /// `f`: flags.
    pub flags: u64,
    /// `m`: the hashmap, `MAPHASH_LEN` bytes per part.
    pub hashmap: Vec<u8>,
    /// `i`, carried opaque.
    pub i: i64,
    /// `l`, carried opaque.
    pub l: i64,
    /// `q`: request id for a response Resource, or nil for a generic Resource.
    pub q: Option<Vec<u8>>,
}

impl Advertisement {
    /// Parse an advertisement from its msgpack map.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = MapReader::new(bytes);
        let n = r.map_header()?;

        let mut transfer_size = None;
        let mut data_size = None;
        let mut parts = None;
        let mut resource_hash = None;
        let mut original_hash = None;
        let mut random_hash = None;
        let mut flags = None;
        let mut hashmap = None;
        let mut i = None;
        let mut l = None;
        let mut q = None;

        for _ in 0..n {
            let key = r.str_key()?;
            match key {
                b't' => transfer_size = Some(r.uint()?),
                b'd' => data_size = Some(r.uint()?),
                b'n' => parts = Some(r.uint()?),
                b'h' => resource_hash = Some(r.bin()?.to_vec()),
                b'o' => original_hash = Some(r.bin()?.to_vec()),
                b'r' => random_hash = Some(r.bin()?.to_vec()),
                b'f' => flags = Some(r.uint()?),
                b'm' => hashmap = Some(r.bin()?.to_vec()),
                b'i' => i = Some(r.int()?),
                b'l' => l = Some(r.int()?),
                b'q' => q = r.bin_or_nil()?.map(Vec::from),
                _ => r.skip_value()?,
            }
        }

        Ok(Self {
            transfer_size: transfer_size.ok_or(Error::BadRequest)?,
            data_size: data_size.ok_or(Error::BadRequest)?,
            parts: parts.ok_or(Error::BadRequest)?,
            resource_hash: resource_hash.ok_or(Error::BadRequest)?,
            original_hash: original_hash.ok_or(Error::BadRequest)?,
            random_hash: random_hash.ok_or(Error::BadRequest)?,
            flags: flags.ok_or(Error::BadRequest)?,
            hashmap: hashmap.ok_or(Error::BadRequest)?,
            i: i.ok_or(Error::BadRequest)?,
            l: l.ok_or(Error::BadRequest)?,
            q,
        })
    }

    /// Serialise to the msgpack map, in RNS's key order.
    pub fn pack(&self) -> Vec<u8> {
        let mut w = MapWriter::new(11);
        w.str_key(b't');
        w.uint(self.transfer_size);
        w.str_key(b'd');
        w.uint(self.data_size);
        w.str_key(b'n');
        w.uint(self.parts);
        w.str_key(b'h');
        w.bin(&self.resource_hash);
        w.str_key(b'r');
        w.bin(&self.random_hash);
        w.str_key(b'o');
        w.bin(&self.original_hash);
        w.str_key(b'i');
        w.int(self.i);
        w.str_key(b'l');
        w.int(self.l);
        w.str_key(b'q');
        match &self.q {
            Some(v) => w.bin(v),
            None => w.nil(),
        }
        w.str_key(b'f');
        w.uint(self.flags);
        w.str_key(b'm');
        w.bin(&self.hashmap);
        w.finish()
    }

    /// The number of parts named in the hashmap.
    pub fn hashmap_parts(&self) -> usize {
        self.hashmap.len() / MAPHASH_LEN
    }

    /// Whether the data is preceded by metadata ([`FLAG_METADATA`]).
    pub fn has_metadata(&self) -> bool {
        self.flags & FLAG_METADATA != 0
    }
}
