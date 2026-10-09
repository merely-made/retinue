//! Destination names and their hashes.
//!
//! A destination is named by an app name and a list of aspects. They join with `.` into an
//! *expanded name*, e.g. `("retinue", ["test"])` becomes `retinue.test`. Two hashes fall
//! out of it, and they are not interchangeable:
//!
//! ```text
//! name_hash        = trunc10(SHA256(expanded_name))
//! destination_hash = trunc16(SHA256(name_hash || identity_hash))
//! ```
//!
//! Note the second is a hash of two hashes, and that the identity participates only
//! through its own hash. Verified against RNS 1.3.8: the fixture vector reproduces
//! `example_utilities.announcesample.fruits` as `2419dca3c93718497b91990373df1503`.

// Needed by the test build or the tokio shell; the bare no_std lib does not reach it.
#[allow(unused_imports)]
use alloc::vec::Vec;

use alloc::string::String;

use crate::hash::{AddressHash, NameHash};
use crate::identity::Identity;

/// The name of a destination: an app name plus dotted aspects.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DestinationName {
    expanded: String,
    name_hash: NameHash,
}

impl DestinationName {
    /// Build from an app name and aspects. They are joined with `.`.
    ///
    /// `DestinationName::new("retinue", ["test"])` expands to `retinue.test`.
    ///
    /// # Panics
    ///
    /// If the app name or an aspect contains a dot, as RNS raises (`Destination.py` 104-107,
    /// 151). [`try_new`](Self::try_new) returns `None` instead.
    pub fn new<I, S>(app_name: &str, aspects: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::try_new(app_name, aspects).expect("dots can't be used in app names or aspects")
    }

    /// [`new`](Self::new), or `None` if the app name or an aspect contains a dot.
    pub fn try_new<I, S>(app_name: &str, aspects: I) -> Option<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if app_name.contains('.') {
            return None;
        }
        let mut expanded = String::from(app_name);
        for aspect in aspects {
            let aspect = aspect.as_ref();
            if aspect.contains('.') {
                return None;
            }
            expanded.push('.');
            expanded.push_str(aspect);
        }
        let name_hash = NameHash::of(expanded.as_bytes());
        Some(Self {
            expanded,
            name_hash,
        })
    }

    /// The dotted name, e.g. `retinue.test`.
    pub fn expanded(&self) -> &str {
        &self.expanded
    }

    /// `trunc10(SHA256(expanded_name))`. This, not the full digest, is what the wire
    /// carries and what lookups must key on.
    pub fn name_hash(&self) -> NameHash {
        self.name_hash
    }

    /// The destination hash for this name under a given identity.
    pub fn destination_hash(&self, identity: &Identity) -> AddressHash {
        destination_hash(self.name_hash, identity.hash())
    }
}

impl core::fmt::Debug for DestinationName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "DestinationName({})", self.expanded)
    }
}

/// `trunc16(SHA256(name_hash || identity_hash))`.
///
/// Exposed separately from [`DestinationName::destination_hash`] because a receiver
/// validating an announce has the two hashes but not the name they came from: the wire
/// carries the name *hash*, and the plaintext name is unrecoverable.
pub fn destination_hash(name_hash: NameHash, identity_hash: AddressHash) -> AddressHash {
    let mut buf = [0u8; crate::hash::NAME_HASH_LEN + crate::hash::ADDRESS_HASH_LEN];
    buf[..crate::hash::NAME_HASH_LEN].copy_from_slice(name_hash.as_slice());
    buf[crate::hash::NAME_HASH_LEN..].copy_from_slice(identity_hash.as_slice());
    AddressHash::of(&buf)
}

impl DestinationName {
    /// The hash of a *plain* destination: one with no identity, like the well-known
    /// `rnstransport.path.request`. It is `trunc16(SHA256(name_hash))`, the identity-bearing
    /// form with an empty identity. Verified against RNS 1.3.8.
    pub fn plain_hash(&self) -> AddressHash {
        AddressHash::of(self.name_hash.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspects_join_with_dots() {
        let n = DestinationName::new("example_utilities", ["announcesample", "fruits"]);
        assert_eq!(n.expanded(), "example_utilities.announcesample.fruits");
    }

    #[test]
    fn dots_are_refused_in_app_names_and_aspects() {
        assert!(DestinationName::try_new("a.b", ["c"]).is_none());
        assert!(DestinationName::try_new("a", ["b.c"]).is_none());
        assert_eq!(
            DestinationName::try_new("a", ["b", "c"])
                .unwrap()
                .expanded(),
            "a.b.c"
        );
    }

    #[test]
    #[should_panic(expected = "dots")]
    fn new_panics_on_a_dotted_aspect() {
        let _ = DestinationName::new("a", ["b.c"]);
    }

    #[test]
    fn no_aspects_is_just_the_app_name() {
        let n = DestinationName::new("retinue", Vec::<&str>::new());
        assert_eq!(n.expanded(), "retinue");
    }
}
