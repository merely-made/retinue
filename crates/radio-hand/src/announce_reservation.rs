//! Durable announce-timebase reservations, independent of board flash.
//!
//! This is the *body* stored in its own [`crate::store`] A/B pair.  It must
//! not be appended to [`crate::settings::Settings`]: a reservation is
//! disposable protocol state, while settings hold the device identity that
//! survives downgrade and recovery.  The outer store supplies the sequence,
//! checksum, and torn-write recovery; this module gives its opaque body a
//! small, versioned meaning.
//!
//! A board plans a new ceiling from the last durable ceiling, writes
//! [`ReservationPlan::encoded_body`] to the inactive outer slot, reads the
//! authoritative body back, and only then calls
//! [`ReservationPlan::verify_readback`].  The returned [`ActiveLease`] is the
//! only value suitable for constructing Retinue's timebase generator.  A
//! failed or mismatched readback therefore has no usable lease to fall back
//! to.

/// Magic for a reservation body.  It is deliberately distinct from the
/// `RHS0` outer slot magic in [`crate::store`].
pub const BODY_MAGIC: [u8; 4] = *b"RHR0";

/// Reservation-body version read and written by this build.
pub const BODY_VERSION: u16 = 1;

/// Bytes in a reservation body.
///
/// ```text
/// 0..4   magic "RHR0"
/// 4..6   version, u16 LE
/// 6      reserved, zero
/// 7..12  reserved-through, inclusive u40 BE
/// ```
pub const BODY_LEN: usize = 12;

/// The largest ordinal that fits Reticulum's five-byte announce timebase.
///
/// Kept here rather than importing `retinue` so the default `radio-hand`
/// build remains radio-free.  It must stay equal to
/// `retinue::announce::ANNOUNCE_TIMEBASE_MAX`.
pub const TIMEBASE_MAX: u64 = (1_u64 << 40) - 1;

/// Default count of ordinals reserved durably at each node boot.
///
/// Boards may select a smaller or larger nonzero value to trade flash erase
/// cadence against the amount of unused ordinal space lost on a power cut.
pub const DEFAULT_LEASE: u64 = 65_536;

/// The reservation state recovered from the active A/B record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationState {
    /// No reservation record exists yet.  It is safe only before the first
    /// reservation has been committed and verified.
    Uncommissioned,
    /// The inclusive durable ceiling.  A reboot must begin strictly above it.
    ReservedThrough(u64),
}

impl ReservationState {
    /// The state corresponding to an erased, never-written reservation pair.
    pub const fn uncommissioned() -> Self {
        Self::Uncommissioned
    }

    /// Construct a checked durable ceiling for tests and board adapters.
    pub const fn reserved_through(through: u64) -> Result<Self, CeilingError> {
        if through > TIMEBASE_MAX {
            return Err(CeilingError::OutOfRange);
        }
        Ok(Self::ReservedThrough(through))
    }

    /// Decode the active outer-record body.
    ///
    /// `None` is permitted only after the board adapter has proved that both
    /// outer slots are erased or otherwise never commissioned.  In particular,
    /// an outer `Truncated`, `BadCrc`, `BadMagic`, or unsupported-version state
    /// must fault closed and must not be collapsed into `None`. A supplied
    /// body came from an active outer record, so even all-erased bytes there
    /// are corrupt rather than a new board. Callers must fail closed rather
    /// than silently reuse an ordinal.
    pub fn decode(body: Option<&[u8]>) -> Result<Self, DecodeError> {
        let Some(body) = body else {
            return Ok(Self::Uncommissioned);
        };
        if body.len() != BODY_LEN {
            return Err(DecodeError::BadLength {
                expected: BODY_LEN,
                actual: body.len(),
            });
        }
        if body.iter().all(|byte| *byte == 0xFF) {
            return Err(DecodeError::ErasedBody);
        }
        if body[..4] != BODY_MAGIC {
            return Err(DecodeError::BadMagic);
        }

        let version = u16::from_le_bytes([body[4], body[5]]);
        if version != BODY_VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        if body[6] != 0 {
            return Err(DecodeError::ReservedByte(body[6]));
        }

        let through = ((body[7] as u64) << 32)
            | ((body[8] as u64) << 24)
            | ((body[9] as u64) << 16)
            | ((body[10] as u64) << 8)
            | body[11] as u64;
        // Five bytes are necessarily a valid u40.  Keep this checked
        // constructor at the boundary so widening the format cannot quietly
        // turn a malformed value into an emission authority.
        Self::reserved_through(through).map_err(|_| DecodeError::OutOfRange)
    }

    /// The durable ceiling, when this is a commissioned record.
    pub const fn through(self) -> Option<u64> {
        match self {
            Self::Uncommissioned => None,
            Self::ReservedThrough(through) => Some(through),
        }
    }
}

/// Why a caller-provided timebase ceiling cannot be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeilingError {
    /// The value exceeds Reticulum's five-byte announce-timebase field.
    OutOfRange,
}

/// Why a nonblank reservation body cannot be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// A reservation body has a fixed format and length.
    BadLength { expected: usize, actual: usize },
    /// A valid outer record cannot contain an erased reservation body.
    ErasedBody,
    /// The body belongs to another record kind or was damaged.
    BadMagic,
    /// This build intentionally does not interpret a future record version.
    UnsupportedVersion(u16),
    /// A reserved byte was written by an incompatible future format or damaged.
    ReservedByte(u8),
    /// Defensive boundary for a future body format wider than five bytes.
    OutOfRange,
}

/// Why a new durable lease cannot be planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    /// A zero-length lease would create a record from which nothing can emit.
    ZeroLease,
    /// Extending the durable ceiling would exceed the five-byte timebase space.
    Exhausted,
}

/// A reservation that has been calculated but is not yet safe to use.
///
/// This represents the required write-before-use boundary.  It contains the
/// old ceiling as the generator floor and the new ceiling that must survive a
/// readback before an announce may be minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservationPlan {
    floor: u64,
    reserved_through: u64,
}

impl ReservationPlan {
    /// Plan the first reservation or extend an existing one.
    ///
    /// The next boot's generator begins at `floor`, deliberately skipping every
    /// ordinal that might have been emitted before the power cut.  On a first
    /// commission the floor is zero, so an absent clock still emits one first.
    pub const fn for_boot(prior: ReservationState, lease: u64) -> Result<Self, PlanError> {
        if lease == 0 {
            return Err(PlanError::ZeroLease);
        }
        let floor = match prior {
            ReservationState::Uncommissioned => 0,
            ReservationState::ReservedThrough(through) => through,
        };
        let Some(reserved_through) = floor.checked_add(lease) else {
            return Err(PlanError::Exhausted);
        };
        if reserved_through > TIMEBASE_MAX {
            return Err(PlanError::Exhausted);
        }
        Ok(Self {
            floor,
            reserved_through,
        })
    }

    /// Plan a boot with [`DEFAULT_LEASE`].
    pub const fn with_default_lease(prior: ReservationState) -> Result<Self, PlanError> {
        Self::for_boot(prior, DEFAULT_LEASE)
    }

    /// The previous durable ceiling.  Pass this as `last_emitted` to the
    /// firmware timebase generator after [`Self::verify_readback`] succeeds.
    pub const fn floor(&self) -> u64 {
        self.floor
    }

    /// The inclusive new ceiling.  Pass this as `reserved_through` to the
    /// firmware timebase generator after [`Self::verify_readback`] succeeds.
    pub const fn reserved_through(&self) -> u64 {
        self.reserved_through
    }

    /// Produce the opaque body that must be committed through the outer A/B
    /// store before this plan can become an [`ActiveLease`].
    pub const fn encoded_body(&self) -> [u8; BODY_LEN] {
        let through = self.reserved_through.to_be_bytes();
        [
            BODY_MAGIC[0],
            BODY_MAGIC[1],
            BODY_MAGIC[2],
            BODY_MAGIC[3],
            BODY_VERSION as u8,
            (BODY_VERSION >> 8) as u8,
            0,
            through[3],
            through[4],
            through[5],
            through[6],
            through[7],
        ]
    }

    /// Accept only the state read back from the newly authoritative outer
    /// record.  A board must not construct an announce generator before this
    /// succeeds.
    pub const fn verify_readback(
        &self,
        readback: ReservationState,
    ) -> Result<ActiveLease, VerifyError> {
        match readback {
            ReservationState::Uncommissioned => Err(VerifyError::Missing),
            ReservationState::ReservedThrough(found) if found != self.reserved_through => {
                Err(VerifyError::CeilingMismatch {
                    expected: self.reserved_through,
                    found,
                })
            }
            ReservationState::ReservedThrough(_) => Ok(ActiveLease {
                floor: self.floor,
                reserved_through: self.reserved_through,
            }),
        }
    }

    /// Decode and verify the authoritative readback in one pure transition.
    pub fn verify_body(&self, body: Option<&[u8]>) -> Result<ActiveLease, VerifyBodyError> {
        let state = ReservationState::decode(body).map_err(VerifyBodyError::Malformed)?;
        self.verify_readback(state)
            .map_err(VerifyBodyError::Mismatch)
    }
}

/// A durable, verified lease from which a board may construct its generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveLease {
    floor: u64,
    reserved_through: u64,
}

impl ActiveLease {
    /// The prior durable ceiling to use as the generator's last emitted value.
    pub const fn floor(&self) -> u64 {
        self.floor
    }

    /// The inclusive ceiling the generator may use until the next reboot lease.
    pub const fn reserved_through(&self) -> u64 {
        self.reserved_through
    }
}

/// Why a parsed reservation readback does not authorize a planned lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// The new record was absent after the board reported a successful write.
    Missing,
    /// The record exists but does not contain the ceiling just planned.
    CeilingMismatch { expected: u64, found: u64 },
}

/// Why raw readback bytes do not authorize a planned lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyBodyError {
    /// The body is nonblank but malformed, unsupported, or out of range.
    Malformed(DecodeError),
    /// The readable state was absent or belonged to another lease.
    Mismatch(VerifyError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_proven_blank_outer_pair_is_uncommissioned() {
        assert_eq!(
            ReservationState::decode(None),
            Ok(ReservationState::Uncommissioned)
        );
        assert_eq!(
            ReservationState::decode(Some(&[0xFF; BODY_LEN])),
            Err(DecodeError::ErasedBody)
        );
    }

    #[test]
    fn valid_body_preserves_the_big_endian_u40_ceiling() {
        let plan = ReservationPlan::for_boot(ReservationState::Uncommissioned, 0x01_02_03_04_05)
            .expect("representable lease");
        let body = plan.encoded_body();

        assert_eq!(&body[7..], &[0x01, 0x02, 0x03, 0x04, 0x05]);
        assert_eq!(
            ReservationState::decode(Some(&body)),
            Ok(ReservationState::ReservedThrough(0x01_02_03_04_05))
        );
    }

    #[test]
    fn a_nonblank_body_with_wrong_magic_is_not_a_new_board() {
        let mut body = ReservationPlan::with_default_lease(ReservationState::Uncommissioned)
            .unwrap()
            .encoded_body();
        body[0] ^= 0xFF;
        assert_eq!(
            ReservationState::decode(Some(&body)),
            Err(DecodeError::BadMagic)
        );
    }

    #[test]
    fn a_future_version_is_refused() {
        let mut body = ReservationPlan::with_default_lease(ReservationState::Uncommissioned)
            .unwrap()
            .encoded_body();
        body[4..6].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(
            ReservationState::decode(Some(&body)),
            Err(DecodeError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn nonzero_reserved_byte_is_refused() {
        let mut body = ReservationPlan::with_default_lease(ReservationState::Uncommissioned)
            .unwrap()
            .encoded_body();
        body[6] = 1;
        assert_eq!(
            ReservationState::decode(Some(&body)),
            Err(DecodeError::ReservedByte(1))
        );
    }

    #[test]
    fn body_length_is_part_of_the_format() {
        assert_eq!(
            ReservationState::decode(Some(&[])),
            Err(DecodeError::BadLength {
                expected: BODY_LEN,
                actual: 0,
            })
        );
    }

    #[test]
    fn checked_constructor_rejects_values_outside_u40() {
        assert_eq!(
            ReservationState::reserved_through(TIMEBASE_MAX + 1),
            Err(CeilingError::OutOfRange)
        );
    }

    #[test]
    fn first_commission_reserves_default_lease_above_zero() {
        let plan = ReservationPlan::with_default_lease(ReservationState::Uncommissioned).unwrap();
        assert_eq!(plan.floor(), 0);
        assert_eq!(plan.reserved_through(), DEFAULT_LEASE);
    }

    #[test]
    fn next_boot_starts_above_the_old_durable_ceiling() {
        let prior = ReservationState::reserved_through(42).unwrap();
        let plan = ReservationPlan::for_boot(prior, 9).unwrap();
        assert_eq!(plan.floor(), 42);
        assert_eq!(plan.reserved_through(), 51);
    }

    #[test]
    fn zero_lease_and_u40_exhaustion_fail_closed() {
        assert_eq!(
            ReservationPlan::for_boot(ReservationState::Uncommissioned, 0),
            Err(PlanError::ZeroLease)
        );
        assert_eq!(
            ReservationPlan::for_boot(ReservationState::ReservedThrough(TIMEBASE_MAX), 1),
            Err(PlanError::Exhausted)
        );
        assert_eq!(
            ReservationPlan::for_boot(ReservationState::ReservedThrough(TIMEBASE_MAX - 1), 2),
            Err(PlanError::Exhausted)
        );
    }

    #[test]
    fn planned_lease_requires_exact_readback() {
        let plan = ReservationPlan::for_boot(ReservationState::ReservedThrough(10), 5).unwrap();
        let body = plan.encoded_body();
        let active = plan
            .verify_body(Some(&body))
            .expect("exact durable readback");
        assert_eq!(active.floor(), 10);
        assert_eq!(active.reserved_through(), 15);

        assert_eq!(
            plan.verify_readback(ReservationState::Uncommissioned),
            Err(VerifyError::Missing)
        );
        assert_eq!(
            plan.verify_readback(ReservationState::ReservedThrough(14)),
            Err(VerifyError::CeilingMismatch {
                expected: 15,
                found: 14,
            })
        );
    }

    #[test]
    fn malformed_readback_cannot_become_a_lease() {
        let plan = ReservationPlan::with_default_lease(ReservationState::Uncommissioned).unwrap();
        let mut body = plan.encoded_body();
        body[6] = 4;
        assert_eq!(
            plan.verify_body(Some(&body)),
            Err(VerifyBodyError::Malformed(DecodeError::ReservedByte(4)))
        );
    }
}
