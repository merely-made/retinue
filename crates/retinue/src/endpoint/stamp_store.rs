//! Destination stamps kept across restarts, in RNS 1.6.1's `storagepath/stamps` layout.
//!
//! Adapted from RNS Stamps.py (`store_destination_stamp`, `load_destination_stamp`),
//! copyright 2016-2026 Mark Qvist, under the Reticulum License. See this crate's LICENSE
//! and NOTICE.
//!
//! Each file is `<destination hex>.dstamp`, a MessagePack map
//! `{"v": 1, "stamp": bin, "value": float, "generated": float}`. A stamp mined by stock
//! RNS on another machine can be copied in, which is how RNS expects a slow device to
//! get its first stamp.

use crate::AddressHash;
use crate::network_stamp::STAMP_LEN;
use alloc::format;
use alloc::vec::Vec;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where an endpoint keeps the stamps it mined for its own destinations. Loaded stamps
/// are validated against the current material and target before use.
pub trait DestinationStampStore: Send + Sync {
    fn load(&self, destination: AddressHash) -> Option<[u8; STAMP_LEN]>;
    /// Keep a newly mined stamp and its value.
    ///
    /// # Errors
    ///
    /// Any write failure. The endpoint announces anyway; the next restart mines again.
    fn store(
        &self,
        destination: AddressHash,
        stamp: &[u8; STAMP_LEN],
        value: f64,
    ) -> io::Result<()>;
}

const EXTENSION: &str = "dstamp";
const FILE_VERSION: u64 = 1;

/// A directory of `.dstamp` files, shared with stock RNS when it is `storagepath/stamps`.
#[derive(Clone, Debug)]
pub struct StampDirectory {
    dir: PathBuf,
}

impl StampDirectory {
    /// The `stamps` directory under an RNS-style storage path.
    pub fn in_storage(storage: &Path) -> Self {
        Self::at(storage.join("stamps"))
    }
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn file(&self, destination: AddressHash) -> PathBuf {
        self.dir.join(format!("{destination}.{EXTENSION}"))
    }
}

impl DestinationStampStore for StampDirectory {
    fn load(&self, destination: AddressHash) -> Option<[u8; STAMP_LEN]> {
        decode(&std::fs::read(self.file(destination)).ok()?)
    }
    fn store(
        &self,
        destination: AddressHash,
        stamp: &[u8; STAMP_LEN],
        value: f64,
    ) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let generated = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        let path = self.file(destination);
        let temp = path.with_extension(format!("{EXTENSION}.{}.tmp", std::process::id()));
        std::fs::write(&temp, encode(stamp, value, generated))?;
        std::fs::rename(&temp, &path)
    }
}

/// Stock's key order, as `msgpack.packb` writes the dict.
fn encode(stamp: &[u8; STAMP_LEN], value: f64, generated: f64) -> Vec<u8> {
    let mut out = Vec::from([0x84]);
    out.extend_from_slice(b"\xa1v");
    out.push(FILE_VERSION as u8);
    out.extend_from_slice(b"\xa5stamp\xc4");
    out.push(STAMP_LEN as u8);
    out.extend_from_slice(stamp);
    out.extend_from_slice(b"\xa5value\xcb");
    out.extend_from_slice(&value.to_be_bytes());
    out.extend_from_slice(b"\xa9generated\xcb");
    out.extend_from_slice(&generated.to_be_bytes());
    out
}

/// Read `v` and `stamp` from a map in any key order; other values are skipped. Like
/// stock, the stored value is not trusted: the caller revalidates the stamp.
fn decode(bytes: &[u8]) -> Option<[u8; STAMP_LEN]> {
    let mut reader = Reader { bytes, at: 0 };
    let entries = match reader.byte()? {
        b @ 0x80..=0x8f => usize::from(b & 0x0f),
        0xde => usize::from(u16::from_be_bytes(reader.take()?)),
        _ => return None,
    };
    let (mut version, mut stamp) = (None, None);
    for _ in 0..entries {
        let key = reader.string()?;
        match key {
            b"v" => version = Some(reader.uint()?),
            b"stamp" => stamp = Some(reader.bin()?),
            _ => reader.skip()?,
        }
    }
    if version != Some(FILE_VERSION) || reader.at != bytes.len() {
        return None;
    }
    stamp?.try_into().ok()
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn slice(&mut self, len: usize) -> Option<&'a [u8]> {
        let out = self.bytes.get(self.at..self.at.checked_add(len)?)?;
        self.at += len;
        Some(out)
    }
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.slice(N)?.try_into().ok()
    }
    fn byte(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        let len = match self.byte()? {
            b @ 0xa0..=0xbf => usize::from(b & 0x1f),
            0xd9 => usize::from(self.byte()?),
            _ => return None,
        };
        self.slice(len)
    }
    fn bin(&mut self) -> Option<&'a [u8]> {
        let len = match self.byte()? {
            0xc4 => usize::from(self.byte()?),
            0xc5 => usize::from(u16::from_be_bytes(self.take()?)),
            _ => return None,
        };
        self.slice(len)
    }
    fn uint(&mut self) -> Option<u64> {
        Some(match self.byte()? {
            b @ 0x00..=0x7f => u64::from(b),
            0xcc => u64::from(self.byte()?),
            0xcd => u64::from(u16::from_be_bytes(self.take()?)),
            0xce => u64::from(u32::from_be_bytes(self.take()?)),
            0xcf => u64::from_be_bytes(self.take()?),
            _ => return None,
        })
    }
    /// Scalars only: a stamp file holds no nested values.
    fn skip(&mut self) -> Option<()> {
        let len = match self.bytes.get(self.at)? {
            0x00..=0x7f | 0xe0..=0xff | 0xc0 | 0xc2 | 0xc3 => 1,
            0xa0..=0xbf | 0xd9 => return self.string().map(drop),
            0xc4 | 0xc5 => return self.bin().map(drop),
            0xcc | 0xd0 => 2,
            0xcd | 0xd1 => 3,
            0xca | 0xce | 0xd2 => 5,
            0xcb | 0xcf | 0xd3 => 9,
            _ => return None,
        };
        self.slice(len).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RNS.vendor.umsgpack.packb` from RNS 1.6.1, as `store_destination_stamp` calls it.
    const STOCK: &str = "84a17601a57374616d70c40e0707070707070707070707070707a576616c7565cb4039800000000000a967656e657261746564cb3ff0000000000000";

    #[test]
    fn writes_stock_bytes() {
        let stock: Vec<u8> = (0..STOCK.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&STOCK[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(encode(&[7; STAMP_LEN], 25.5, 1.0), stock);
    }

    #[test]
    fn round_trips_and_rejects_other_versions_and_lengths() {
        let stamp = [7; STAMP_LEN];
        let bytes = encode(&stamp, 25.5, 1.0);
        assert_eq!(decode(&bytes), Some(stamp));
        let mut other = bytes.clone();
        other[3] = 2;
        assert_eq!(decode(&other), None);
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        let mut short = encode(&stamp, 25.5, 1.0);
        short[11] = 13;
        assert_eq!(decode(&short), None);
    }

    #[test]
    fn directory_store_writes_the_stock_file_name() {
        let root = std::env::temp_dir().join(format!("retinue-dstamp-{}", std::process::id()));
        let store = StampDirectory::in_storage(&root);
        let destination = AddressHash::from_bytes([0xab; 16]);
        assert!(store.load(destination).is_none());
        store.store(destination, &[3; STAMP_LEN], 25.0).unwrap();
        assert_eq!(
            store.file(destination).file_name().unwrap(),
            "abababababababababababababababab.dstamp"
        );
        assert_eq!(store.load(destination), Some([3; STAMP_LEN]));
        std::fs::remove_dir_all(root).unwrap();
    }
}
