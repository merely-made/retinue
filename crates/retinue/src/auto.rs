//! AutoInterface addressing, sans-io: the discovery group, the link-local text a peering
//! token hashes, and a ring of recent frame hashes (`AutoInterface.py` 47-73, 80-85,
//! 203-213, 369-371, 490-513, 649-665).
//!
//! Every byte here is matched by stock peers: a token over the wrong text is dropped at
//! DEBUG level, so nothing ever peers. The socket side is `iface::auto`.

use core::fmt::Write;
use core::net::Ipv6Addr;

use sha2::{Digest, Sha256};

/// The default discovery group id (`AutoInterface.py` 49).
pub const DEFAULT_GROUP: &[u8] = b"reticulum";
/// Multicast discovery port; unicast (reverse) discovery is the next one (`AutoInterface.py` 47, 174).
pub const DISCOVERY_PORT: u16 = 29716;
/// The per-interface data port (`AutoInterface.py` 48).
pub const DATA_PORT: u16 = 42671;
/// A peering token is one SHA-256 digest.
pub const TOKEN_LEN: usize = 32;

/// Multicast scope of the discovery group (`AutoInterface.py` 52-56).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Link,
    Admin,
    Site,
    Organisation,
    Global,
}

impl Scope {
    pub const ALL: [Self; 5] = [
        Self::Link,
        Self::Admin,
        Self::Site,
        Self::Organisation,
        Self::Global,
    ];

    /// The scope nibble of the group address.
    pub const fn nibble(self) -> u16 {
        match self {
            Self::Link => 0x2,
            Self::Admin => 0x4,
            Self::Site => 0x5,
            Self::Organisation => 0x8,
            Self::Global => 0xe,
        }
    }

    /// The RNS `discovery_scope` value, any case. `None` for an unknown one, which stock
    /// fails on with AttributeError (`AutoInterface.py` 190-201, 213).
    pub fn from_config(value: &str) -> Option<Self> {
        let names = ["link", "admin", "site", "organisation", "global"];
        let i = names.iter().position(|n| value.eq_ignore_ascii_case(n))?;
        Some(Self::ALL[i])
    }
}

/// Multicast address type of the discovery group (`AutoInterface.py` 58-59).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddrType {
    #[default]
    Temporary,
    Permanent,
}

impl AddrType {
    /// The flags nibble of the group address.
    pub const fn nibble(self) -> u16 {
        match self {
            Self::Temporary => 0x1,
            Self::Permanent => 0x0,
        }
    }

    /// The RNS `multicast_address_type` value; anything unknown is temporary, as stock
    /// falls back (`AutoInterface.py` 176-183).
    pub fn from_config(value: &str) -> Self {
        if value.eq_ignore_ascii_case("permanent") {
            Self::Permanent
        } else {
            Self::Temporary
        }
    }
}

/// The discovery group: `ff<type><scope>:0:` then SHA-256(group) bytes 2..14 as six
/// big-endian hextets (`AutoInterface.py` 203-213). The default is
/// `ff12:0:d70b:fb1c:16e4:5e39:485e:31e1`.
pub fn discovery_group(group: &[u8], scope: Scope, kind: AddrType) -> Ipv6Addr {
    let g = Sha256::digest(group);
    let h = |i: usize| u16::from_be_bytes([g[i], g[i + 1]]);
    let prefix = 0xff00 | kind.nibble() << 4 | scope.nibble();
    Ipv6Addr::new(prefix, 0, h(2), h(4), h(6), h(8), h(10), h(12))
}

/// A link-local address as peers see it from `recvfrom`: the BSD/macOS KAME scope that
/// `getifaddrs` embeds in hextet 1 cleared. Unlike stock's regex (`AutoInterface.py` 84),
/// this also clears it when the interface id holds the longest zero run (`fe80:4:0:0:1::`).
pub fn descope(addr: Ipv6Addr) -> Ipv6Addr {
    let mut s = addr.segments();
    if s[0] == 0xfe80 {
        s[1] = 0;
    }
    Ipv6Addr::from(s)
}

/// SHA-256(group ‖ RFC 5952 text of `addr`), the token a node multicasts and unicasts for
/// its link-local address (`AutoInterface.py` 493, 507). Rust's `Ipv6Addr` display is the
/// same text as Python's `inet_ntop` and `ipaddress` for these addresses.
pub fn peering_token(group: &[u8], addr: &Ipv6Addr) -> [u8; TOKEN_LEN] {
    let mut text = heapless::String::<39>::new();
    write!(text, "{addr}").expect("an IPv6 address is at most 39 characters");
    let mut hash = Sha256::new();
    hash.update(group);
    hash.update(text.as_bytes());
    hash.finalize().into()
}

/// Whether `datagram` starts with `src`'s token; trailing bytes are ignored, as stock
/// compares only the first 32 (`AutoInterface.py` 369-371).
pub fn token_valid(group: &[u8], src: &Ipv6Addr, datagram: &[u8]) -> bool {
    datagram.get(..TOKEN_LEN) == Some(&peering_token(group, src)[..])
}

/// The last `N` frame hashes with when they were seen: Auto's multi-interface dedup
/// (48 entries, `AutoInterface.py` 72-73, 649-665) and UDP's own-broadcast echo guard.
/// Times are caller milliseconds; the ring holds no clock.
#[derive(Clone, Debug)]
pub struct HashRing<const N: usize> {
    entries: [Option<([u8; 32], u64)>; N],
    next: usize,
}

impl<const N: usize> Default for HashRing<N> {
    fn default() -> Self {
        Self {
            entries: [None; N],
            next: 0,
        }
    }
}

impl<const N: usize> HashRing<N> {
    /// Whether `hash` was recorded within `ttl_ms` before `now_ms`.
    pub fn fresh(&self, hash: &[u8; 32], now_ms: u64, ttl_ms: u64) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|(h, at)| h == hash && now_ms < at.saturating_add(ttl_ms))
    }

    /// Record `hash`, evicting the oldest entry once full.
    pub fn insert(&mut self, hash: [u8; 32], now_ms: u64) {
        if N == 0 {
            return;
        }
        self.entries[self.next] = Some((hash, now_ms));
        self.next = (self.next + 1) % N;
    }
}

#[cfg(test)]
mod tests;
