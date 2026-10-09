//! Node state, contacts and learned routes.

use alloc::vec::Vec;

use super::{CapacityError, DirectRoute, NodeCapacity};
use crate::identity::{Identity, LocalIdentity};
use crate::mesh::SeenTable;
use crate::message::TextMessage;

pub(super) struct Contact {
    pub(super) identity: Identity,
    pub(super) route: Option<DirectRoute>,
}

/// Something the node surfaces to the application from a received frame.
#[derive(Debug, Clone)]
pub enum Event {
    /// A contact advertised itself (a new or refreshed identity).
    Advert {
        identity: Identity,
        timestamp: u32,
        app_data: Vec<u8>,
    },
    /// A text message addressed to us, decrypted, with the ack we should return.
    Message {
        /// The sender's 1-byte node hash.
        from: u8,
        message: TextMessage,
        ack: [u8; 4],
    },
    /// An ack floated past (whoever is awaiting it matches the 4 bytes).
    Ack([u8; 4]),
}

/// A MeshCore node.
pub struct Node {
    pub(super) identity: LocalIdentity,
    pub(super) me: Identity,
    pub(super) seen: SeenTable,
    pub(super) allow_forward: bool,
    pub(super) flood_hash_size: u8,
    /// Learned contacts, keyed by 1-byte node hash. A collision with a different full identity
    /// is refused, so it cannot replace the existing identity or route.
    pub(super) contacts: Vec<(u8, Contact)>,
    pub(super) capacity: NodeCapacity,
}

impl Node {
    /// A node with the given identity. `allow_forward` is `true` for a repeater (retransmits
    /// others' traffic), `false` for a leaf.
    pub fn new(identity: LocalIdentity, allow_forward: bool) -> Self {
        Self::with_capacity(identity, allow_forward, NodeCapacity::default())
            .expect("the default Tucket capacity is valid")
    }

    /// Construct a node with explicitly bounded resident contacts and dedup.
    pub fn with_capacity(
        identity: LocalIdentity,
        allow_forward: bool,
        capacity: NodeCapacity,
    ) -> Result<Self, CapacityError> {
        let me = identity.identity();
        let seen = SeenTable::with_capacity(capacity.dedup).ok_or(CapacityError::ZeroCapacity)?;
        Ok(Node {
            identity,
            me,
            seen,
            allow_forward,
            flood_hash_size: 1,
            contacts: Vec::with_capacity(capacity.contacts),
            capacity,
        })
    }

    /// Our identity.
    pub fn identity(&self) -> &Identity {
        &self.me
    }

    /// Our 1-byte node hash.
    pub fn my_hash(&self) -> u8 {
        self.me.hash()[0]
    }

    /// Set the public-key prefix width for newly originated flood paths.
    /// Received paths and learned direct routes retain their own wire width.
    /// Refuses reserved/invalid widths without changing the current setting.
    pub fn set_flood_hash_size(&mut self, size: u8) -> bool {
        if !(1..=3).contains(&size) {
            return false;
        }
        self.flood_hash_size = size;
        true
    }

    pub fn flood_hash_size(&self) -> u8 {
        self.flood_hash_size
    }

    /// A learned contact by node hash.
    pub fn contact(&self, hash: u8) -> Option<&Identity> {
        self.contacts
            .iter()
            .find(|(key, _)| *key == hash)
            .map(|(_, contact)| &contact.identity)
    }

    /// The authenticated direct route currently learned for a contact.
    pub fn route_to(&self, hash: u8) -> Option<&DirectRoute> {
        self.contacts
            .iter()
            .find(|(key, _)| *key == hash)?
            .1
            .route
            .as_ref()
    }

    /// Set an operator-selected route for a known contact, taking precedence
    /// over automatic flood discovery.
    pub fn set_route(&mut self, hash: u8, route: DirectRoute) -> bool {
        let Some((_, contact)) = self.contacts.iter_mut().find(|(key, _)| *key == hash) else {
            return false;
        };
        contact.route = Some(route);
        true
    }

    /// Forget a route after a failed direct delivery so the next send floods and discovers a
    /// fresh one.
    pub fn clear_route(&mut self, hash: u8) {
        if let Some((_, contact)) = self.contacts.iter_mut().find(|(key, _)| *key == hash) {
            contact.route = None;
        }
    }

    /// Add a contact or refresh an identical identity without evicting an unrelated one.
    ///
    /// A different identity with an occupied one-byte wire hash is refused. The current wire
    /// addressing remains one byte, so this does not resolve the ambiguity on the network; it
    /// only preserves the contact and route already selected locally.
    pub fn try_add_contact(&mut self, identity: Identity) -> Result<(), CapacityError> {
        let hash = identity.hash()[0];
        if let Some((_, contact)) = self.contacts.iter_mut().find(|(key, _)| *key == hash) {
            return if contact.identity == identity {
                Ok(())
            } else {
                Err(CapacityError::ContactHashCollision)
            };
        }
        if self.contacts.len() == self.capacity.contacts {
            return Err(CapacityError::ContactsFull);
        }
        self.contacts.push((
            hash,
            Contact {
                identity,
                route: None,
            },
        ));
        Ok(())
    }
}
