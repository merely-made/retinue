//! A small msgpack map codec, enough for the advertisement and HMU.

use alloc::vec::Vec;

use crate::{Error, Result};

pub(super) struct MapWriter {
    out: Vec<u8>,
}

impl MapWriter {
    pub(super) fn new(entries: usize) -> Self {
        let mut out = Vec::new();
        assert!(entries < 16, "advertisement fits in a fixmap");
        out.push(0x80 | entries as u8);
        Self { out }
    }
    /// A writer for a fixarray of `entries` values.
    pub(super) fn array(entries: usize) -> Self {
        assert!(entries < 16, "fits in a fixarray");
        Self {
            out: alloc::vec![0x90 | entries as u8],
        }
    }
    pub(super) fn str_key(&mut self, k: u8) {
        self.out.push(0xa1); // fixstr, len 1
        self.out.push(k);
    }
    pub(super) fn uint(&mut self, v: u64) {
        if v < 0x80 {
            self.out.push(v as u8);
        } else if v <= u8::MAX as u64 {
            self.out.push(0xcc);
            self.out.push(v as u8);
        } else if v <= u16::MAX as u64 {
            self.out.push(0xcd);
            self.out.extend_from_slice(&(v as u16).to_be_bytes());
        } else if v <= u32::MAX as u64 {
            self.out.push(0xce);
            self.out.extend_from_slice(&(v as u32).to_be_bytes());
        } else {
            self.out.push(0xcf);
            self.out.extend_from_slice(&v.to_be_bytes());
        }
    }
    /// The shortest form, as Python's msgpack packs: a non-negative value as an unsigned.
    pub(super) fn int(&mut self, v: i64) {
        if v >= 0 {
            self.uint(v as u64);
        } else if v >= -32 {
            self.out.push(v as u8); // negative fixint
        } else if v >= i64::from(i8::MIN) {
            self.out.push(0xd0);
            self.out.push(v as u8);
        } else if v >= i64::from(i16::MIN) {
            self.out.push(0xd1);
            self.out.extend_from_slice(&(v as i16).to_be_bytes());
        } else if v >= i64::from(i32::MIN) {
            self.out.push(0xd2);
            self.out.extend_from_slice(&(v as i32).to_be_bytes());
        } else {
            self.out.push(0xd3);
            self.out.extend_from_slice(&v.to_be_bytes());
        }
    }
    pub(super) fn nil(&mut self) {
        self.out.push(0xc0);
    }
    pub(super) fn bin(&mut self, b: &[u8]) {
        match b.len() {
            n if n <= u8::MAX as usize => {
                self.out.push(0xc4);
                self.out.push(n as u8);
            }
            n if n <= u16::MAX as usize => {
                self.out.push(0xc5);
                self.out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                self.out.push(0xc6);
                self.out.extend_from_slice(&(n as u32).to_be_bytes());
            }
        }
        self.out.extend_from_slice(b);
    }
    pub(super) fn finish(self) -> Vec<u8> {
        self.out
    }
}

pub(super) struct MapReader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> MapReader<'a> {
    pub(super) fn new(b: &'a [u8]) -> Self {
        Self { b, i: 0 }
    }
    pub(super) fn byte(&mut self) -> Result<u8> {
        let v = *self.b.get(self.i).ok_or(Error::BadRequest)?;
        self.i += 1;
        Ok(v)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.i..self.i + n).ok_or(Error::BadRequest)?;
        self.i += n;
        Ok(s)
    }
    pub(super) fn map_header(&mut self) -> Result<usize> {
        let t = self.byte()?;
        match t {
            0x80..=0x8f => Ok((t & 0x0f) as usize),
            0xde => {
                let n = self.take(2)?;
                Ok(u16::from_be_bytes([n[0], n[1]]) as usize)
            }
            _ => Err(Error::BadRequest),
        }
    }
    pub(super) fn str_key(&mut self) -> Result<u8> {
        let t = self.byte()?;
        // Only single-letter keys appear here.
        if (0xa1..=0xbf).contains(&t) {
            let len = (t & 0x1f) as usize;
            let s = self.take(len)?;
            Ok(s[0])
        } else {
            Err(Error::BadRequest)
        }
    }
    pub(super) fn uint(&mut self) -> Result<u64> {
        let t = self.byte()?;
        Ok(match t {
            0x00..=0x7f => t as u64,
            0xcc => self.byte()? as u64,
            0xcd => {
                let n = self.take(2)?;
                u16::from_be_bytes([n[0], n[1]]) as u64
            }
            0xce => {
                let n = self.take(4)?;
                u32::from_be_bytes([n[0], n[1], n[2], n[3]]) as u64
            }
            0xcf => {
                let n = self.take(8)?;
                u64::from_be_bytes(n.try_into().expect("8"))
            }
            _ => return Err(Error::BadRequest),
        })
    }
    /// Any msgpack integer that fits an `i64`, in whatever width the peer chose.
    pub(super) fn int(&mut self) -> Result<i64> {
        let t = self.b.get(self.i).copied().ok_or(Error::BadRequest)?;
        if (0xcc..=0xcf).contains(&t) || t < 0x80 {
            return i64::try_from(self.uint()?).map_err(|_| Error::BadRequest);
        }
        self.i += 1;
        Ok(match t {
            0xe0..=0xff => i64::from(t as i8), // negative fixint
            0xd0 => i64::from(self.byte()? as i8),
            0xd1 => i64::from(i16::from_be_bytes(self.take(2)?.try_into().expect("2"))),
            0xd2 => i64::from(i32::from_be_bytes(self.take(4)?.try_into().expect("4"))),
            0xd3 => i64::from_be_bytes(self.take(8)?.try_into().expect("8")),
            _ => return Err(Error::BadRequest),
        })
    }
    pub(super) fn bin_or_nil(&mut self) -> Result<Option<&'a [u8]>> {
        if self.b.get(self.i) == Some(&0xc0) {
            self.i += 1;
            Ok(None)
        } else {
            Ok(Some(self.bin()?))
        }
    }
    pub(super) fn bin(&mut self) -> Result<&'a [u8]> {
        let t = self.byte()?;
        let len = match t {
            0xc4 => self.byte()? as usize,
            0xc5 => {
                let n = self.take(2)?;
                u16::from_be_bytes([n[0], n[1]]) as usize
            }
            0xc6 => {
                let n = self.take(4)?;
                u32::from_be_bytes([n[0], n[1], n[2], n[3]]) as usize
            }
            _ => return Err(Error::BadRequest),
        };
        self.take(len)
    }
    pub(super) fn skip_value(&mut self) -> Result<()> {
        // Only needed if RNS adds keys we do not model; skip common scalar shapes.
        let t = self.byte()?;
        match t {
            0x00..=0x7f | 0xe0..=0xff | 0xc0 => Ok(()),
            0xcc | 0xd0 => {
                self.byte()?;
                Ok(())
            }
            0xcd | 0xd1 => {
                self.take(2)?;
                Ok(())
            }
            0xce | 0xd2 => {
                self.take(4)?;
                Ok(())
            }
            0xcf | 0xd3 => {
                self.take(8)?;
                Ok(())
            }
            0xc4 => {
                let n = self.byte()? as usize;
                self.take(n)?;
                Ok(())
            }
            _ => Err(Error::BadRequest),
        }
    }
}
