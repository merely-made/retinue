//! What the node asks of its shell.

use alloc::vec::Vec;

use heapless::Vec as BoundedVec;

#[cfg(doc)]
use super::{Node, link_request_timeout};
use crate::hash::AddressHash;
use crate::packet::Packet;

/// Which interface a packet arrived on or should leave by.
///
/// A plain integer chosen by the shell, matching the desktop's `InterfaceId`, so a board
/// with one radio and one host link can simply number them.
pub type InterfaceId = u32;

/// Something the node wants the shell to do.
///
/// The node never acts; it decides. A shell reads these and performs them with whatever
/// radio, timer and link it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Put this packet on the wire, by this interface.
    Send {
        interface: InterfaceId,
        packet: Packet,
    },
    /// A destination was learned or refreshed from a valid announce. The shell may show it
    /// on a face or hand it to an application; the node has already recorded it.
    Learned { destination: AddressHash },
    /// A link is established, in either direction. The shell may now carry data on it.
    LinkUp { link_id: AddressHash },
    /// An established link ended, because the peer closed it or the node dropped it.
    LinkDown { link_id: AddressHash },
    /// A link request this node opened got no proof by its deadline (see
    /// [`link_request_timeout`]) and was dropped. That link never came up, so this is not a
    /// [`Action::LinkDown`]. The id is the one `link::link_id` reads from the request.
    ///
    /// Stricter than RNS 1.5.4, which reports the same TIMEOUT reason for a request that was
    /// never answered and for an established link later lost
    /// (`testing/receipts/rns-1.5.4-link-echo-corroboration`, Q1).
    LinkRequestTimedOut { link_id: AddressHash },
    /// Application bytes arrived on a link, already decrypted.
    Data {
        link_id: AddressHash,
        payload: Vec<u8>,
    },
    /// A resource arrived whole, reassembled and verified against its advertised hash.
    ///
    /// Only the data is carried. Metadata the sender attached (RNS's
    /// `Resource(data, metadata=...)`) is split off and not delivered; each such drop is
    /// counted by [`Node::dropped_metadata`].
    Resource { link_id: AddressHash, data: Vec<u8> },
}

/// What one `ingest` or `poll` produced.
///
/// Bounded, because a single call must never be able to demand unbounded work of a shell
/// that has 256 KB. `overflowed` reports honestly when the bound was reached rather than
/// silently dropping, per the plan's rule that a full table stays operational and says so.
#[derive(Debug)]
pub struct Actions<const N: usize> {
    items: BoundedVec<Action, N>,
    overflowed: u16,
}

impl<const N: usize> Actions<N> {
    pub(super) fn new() -> Self {
        Self {
            items: BoundedVec::new(),
            overflowed: 0,
        }
    }

    pub(super) fn push(&mut self, action: Action) -> bool {
        if self.items.push(action).is_err() {
            self.overflowed = self.overflowed.saturating_add(1);
            false
        } else {
            true
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Action> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Actions that did not fit. Nonzero means the shell is not draining fast enough, or
    /// `ACTIONS` is too small for this traffic.
    pub fn overflowed(&self) -> u16 {
        self.overflowed
    }
}

impl<const N: usize> IntoIterator for Actions<N> {
    type Item = Action;
    type IntoIter = <BoundedVec<Action, N> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}
