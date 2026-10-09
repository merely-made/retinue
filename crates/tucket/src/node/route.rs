//! Validated source routes.

use alloc::vec::Vec;

use crate::packet::Packet;

/// A validated source route to one contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectRoute {
    pub(super) path_len: u8,
    pub(super) path: Vec<u8>,
}

impl DirectRoute {
    /// Construct a validated source route.
    ///
    /// `path_len` packs the hash width and hop count; `path` must contain exactly
    /// that many bytes. A zero-hop route is valid and addresses a radio neighbour
    /// directly.
    pub fn new(path_len: u8, path: &[u8]) -> Option<Self> {
        if !Packet::is_valid_path_len(path_len) {
            return None;
        }
        let byte_len = ((path_len >> 6) as usize + 1) * (path_len & 63) as usize;
        (path.len() == byte_len).then(|| Self {
            path_len,
            path: path.to_vec(),
        })
    }

    pub fn path_len(&self) -> u8 {
        self.path_len
    }

    pub fn path(&self) -> &[u8] {
        &self.path
    }
}
