//! Caller-durable packet-ID leases and their extensions.

/// A caller-durable, exclusive packet-ID interval `[start, end)` for one source.
#[derive(Debug, PartialEq, Eq)]
pub struct PacketIdLease {
    pub(super) source: u32,
    start: u32,
    pub(super) end: u32,
    /// The next ID restored from caller-owned durable state.
    pub(super) next: u32,
}

impl PacketIdLease {
    pub const fn new(source: u32, start: u32, end: u32, next: u32) -> Result<Self, LeaseError> {
        if start >= end {
            return Err(LeaseError::EmptyOrReversed { start, end });
        }
        if next < start || next > end {
            return Err(LeaseError::NextOutside { start, end, next });
        }
        Ok(Self {
            source,
            start,
            end,
            next,
        })
    }

    pub const fn source(&self) -> u32 {
        self.source
    }
    pub const fn start(&self) -> u32 {
        self.start
    }
    pub const fn end(&self) -> u32 {
        self.end
    }
    pub const fn next(&self) -> u32 {
        self.next
    }
}

/// A newly durable exclusive interval which extends a prior lease.
#[derive(Debug, PartialEq, Eq)]
pub struct PacketIdReservation {
    pub(super) source: u32,
    pub(super) start: u32,
    pub(super) end: u32,
}

impl PacketIdReservation {
    pub const fn new(source: u32, start: u32, end: u32) -> Result<Self, LeaseError> {
        if start >= end {
            return Err(LeaseError::EmptyOrReversed { start, end });
        }
        Ok(Self { source, start, end })
    }

    pub const fn source(&self) -> u32 {
        self.source
    }
    pub const fn start(&self) -> u32 {
        self.start
    }
    pub const fn end(&self) -> u32 {
        self.end
    }
}

/// Caller assertion authorizing a lease extension.
///
/// This is not authentication and performs no persistence. The board storage
/// owner must create it only after an A/B write and readback, or after a
/// separately trusted caller has established equivalent authority.
#[derive(Debug)]
pub struct ReservationProof(pub(super) ReservationProofKind);

#[derive(Debug)]
pub(super) enum ReservationProofKind {
    DurableAck,
    TrustedCaller,
}

impl ReservationProof {
    pub const fn durable_ack() -> Self {
        Self(ReservationProofKind::DurableAck)
    }
    pub const fn trusted_caller() -> Self {
        Self(ReservationProofKind::TrustedCaller)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    EmptyOrReversed { start: u32, end: u32 },
    NextOutside { start: u32, end: u32, next: u32 },
}
impl core::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyOrReversed { start, end } => {
                write!(f, "packet-ID lease is empty or reversed: {start}..{end}")
            }
            Self::NextOutside { start, end, next } => {
                write!(f, "packet-ID lease next {next} is outside {start}..={end}")
            }
        }
    }
}
impl core::error::Error for LeaseError {}
