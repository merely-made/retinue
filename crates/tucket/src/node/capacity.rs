//! Resident bounds and refusals.

/// A refusal from the bounded core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityError {
    /// The destination has not advertised a public key yet.
    UnknownContact,
    /// Contact state is full and the incoming advert has no existing slot.
    ContactsFull,
    /// A different full identity shares the one-byte hash of a learned contact.
    ContactHashCollision,
    /// A text body cannot fit in one encrypted MeshCore payload.
    TextTooLong,
    /// Advert application data cannot fit in its fixed wire field.
    AdvertDataTooLong,
    /// A caller-owned pending collection is full.
    PendingFull,
    /// The public retry fields must still describe the four-attempt wire
    /// format, even when callers construct the struct directly.
    InvalidRetryPolicy,
    /// A capacity of zero would silently disable a required table.
    ZeroCapacity,
}

/// Resident state chosen by the firmware integrator.
///
/// `contacts` and `dedup` are allocated once by
/// [`Node::with_capacity`](super::Node::with_capacity) and never grow while
/// handling frames. Pending sends live outside the node: use
/// [`PendingTexts`](super::PendingTexts) or an application-owned equivalent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeCapacity {
    pub contacts: usize,
    pub dedup: usize,
}

impl NodeCapacity {
    pub const fn new(contacts: usize, dedup: usize) -> Result<Self, CapacityError> {
        if contacts == 0 || dedup == 0 {
            return Err(CapacityError::ZeroCapacity);
        }
        Ok(Self { contacts, dedup })
    }
}

impl Default for NodeCapacity {
    fn default() -> Self {
        Self {
            contacts: 32,
            dedup: crate::mesh::MAX_PACKET_HASHES,
        }
    }
}
