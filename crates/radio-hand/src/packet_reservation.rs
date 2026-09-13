//! Sennet packet-ID ceilings stored in an independent durable A/B pair.
//! The counter is global to the board, so changing channel/source configuration
//! cannot restart it. A boot discards all IDs below the last durable ceiling.

pub const BODY_LEN: usize = 12;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservationError {
    Corrupt,
    Empty,
    Exhausted,
    WrongReadback,
}

/// Classification of a fully read A/B pair. Erased means every supplied byte
/// is 0xff; a board must supply entire sectors before using the fresh path.
pub struct Snapshot {
    pub state: State,
    pub next: crate::store::Slot,
    pub sequence: u32,
}
pub fn snapshot(a: &[u8], b: &[u8]) -> Result<Snapshot, ReservationError> {
    use crate::store::{self, Slot};
    let mut records = [None, None];
    for (index, bytes) in [a, b].into_iter().enumerate() {
        if bytes.is_empty() {
            return Err(ReservationError::Corrupt);
        }
        if bytes.iter().all(|b| *b == 255) {
            continue;
        }
        let record = store::decode(bytes).map_err(|_| ReservationError::Corrupt)?;
        let state = State::decode(Some(record.body))?;
        records[index] = Some((record.sequence, state.next));
    }
    if let [Some(a), Some(b)] = records {
        let (old, new) = if a.0 < b.0 { (a, b) } else { (b, a) };
        if old.0.checked_add(1) != Some(new.0) || old.1 >= new.1 {
            return Err(ReservationError::Corrupt);
        }
    }
    let selected = store::select(a, b);
    match selected.active {
        None => Ok(Snapshot {
            state: State::decode(None)?,
            next: Slot::A,
            sequence: 0,
        }),
        Some((slot, record)) => Ok(Snapshot {
            state: State::decode(Some(record.body))?,
            next: match slot {
                Slot::A => Slot::B,
                Slot::B => Slot::A,
            },
            sequence: record
                .sequence
                .checked_add(1)
                .ok_or(ReservationError::Exhausted)?,
        }),
    }
}

/// Re-read both complete slots after programming. A lease is issued only if
/// the requested record is now the authoritative pair selection.
pub fn verify_pair(
    plan: Plan,
    a: &[u8],
    b: &[u8],
    slot: crate::store::Slot,
    sequence: u32,
) -> Result<Lease, ReservationError> {
    // A successful maximum sequence would make the next snapshot exhausted;
    // never issue that last record, so every issued lease can be re-read.
    snapshot(a, b)?;
    let (active, record) = crate::store::select(a, b)
        .active
        .ok_or(ReservationError::WrongReadback)?;
    if active != slot || record.sequence != sequence {
        return Err(ReservationError::WrongReadback);
    }
    plan.verify(record.body)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    next: u32,
}
impl State {
    /// Only the flash owner may use `None`, after verifying both slots erased.
    pub fn decode(body: Option<&[u8]>) -> Result<Self, ReservationError> {
        let Some(body) = body else {
            return Ok(Self { next: 1 });
        };
        if body.len() != BODY_LEN || body[..4] != *b"SNP1" || body[8..] != [0; 4] {
            return Err(ReservationError::Corrupt);
        }
        let next = u32::from_le_bytes(body[4..8].try_into().unwrap());
        if next == 0 {
            return Err(ReservationError::Corrupt);
        }
        Ok(Self { next })
    }
    pub fn plan(self, count: u32) -> Result<Plan, ReservationError> {
        if count == 0 {
            return Err(ReservationError::Empty);
        }
        let end = self
            .next
            .checked_add(count)
            .ok_or(ReservationError::Exhausted)?;
        Ok(Plan {
            start: self.next,
            end,
        })
    }
}

pub struct Plan {
    start: u32,
    end: u32,
}
impl Plan {
    pub fn body(&self) -> [u8; BODY_LEN] {
        let mut out = [0; BODY_LEN];
        out[..4].copy_from_slice(b"SNP1");
        out[4..8].copy_from_slice(&self.end.to_le_bytes());
        out
    }
    /// Called only with the authoritative committed slot's exact body.
    pub fn verify(self, body: &[u8]) -> Result<Lease, ReservationError> {
        if body != self.body() {
            return Err(ReservationError::WrongReadback);
        }
        Ok(Lease {
            start: self.start,
            end: self.end,
        })
    }
}

/// Non-copy permission to consume one verified `[start,end)` interval.
#[derive(Debug)]
pub struct Lease {
    start: u32,
    end: u32,
}
impl Lease {
    pub fn start(&self) -> u32 {
        self.start
    }
    pub fn end(&self) -> u32 {
        self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(sequence: u32, ceiling: u32) -> [u8; 64] {
        let mut body = [0; BODY_LEN];
        body[..4].copy_from_slice(b"SNP1");
        body[4..8].copy_from_slice(&ceiling.to_le_bytes());
        let mut slot = [255; 64];
        crate::store::encode(sequence, &body, &mut slot).unwrap();
        slot
    }
    #[test]
    fn pair_refuses_torn_tail_ambiguous_sequence_and_counter_rollback() {
        let blank = [255; 64];
        let a = record(1, 101);
        let mut tail = blank;
        tail[63] = 0;
        assert!(snapshot(&blank, &tail).is_err());
        assert!(snapshot(&a, &record(1, 201)).is_err());
        assert!(snapshot(&a, &record(2, 100)).is_err());
        assert!(snapshot(&record(u32::MAX, 101), &blank).is_err());
        assert_eq!(snapshot(&a, &blank).unwrap().state.next, 101);
    }
    #[test]
    fn authoritative_pair_readback_precedes_lease_release() {
        use crate::store::Slot;
        let blank = [255; 64];
        let a = record(0, 101);
        let prior = snapshot(&a, &blank).unwrap();
        let plan = prior.state.plan(100).unwrap();
        assert!(verify_pair(plan, &a, &blank, Slot::B, 1).is_err());
        let plan = prior.state.plan(100).unwrap();
        let b = record(1, 201);
        let lease = verify_pair(plan, &a, &b, Slot::B, 1).unwrap();
        assert_eq!((lease.start(), lease.end()), (101, 201));
        assert_eq!(snapshot(&a, &b).unwrap().state.plan(1).unwrap().start, 201);
    }
    #[test]
    fn reset_skips_unused_ids_and_readback_is_required() {
        let plan = State::decode(None).unwrap().plan(100).unwrap();
        let body = plan.body();
        assert!(matches!(
            plan.verify(&[0; 12]),
            Err(ReservationError::WrongReadback)
        ));
        let later = State::decode(Some(&body)).unwrap().plan(100).unwrap();
        let nextbody = later.body();
        let lease = later.verify(&nextbody).unwrap();
        assert_eq!((lease.start(), lease.end()), (101, 201));
    }
    #[test]
    fn corrupt_and_exhausted_state_never_restarts() {
        assert!(State::decode(Some(&[255; 12])).is_err());
        let mut body = *b"SNP1xxxxxxxx";
        body[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        body[8..].fill(0);
        assert!(matches!(
            State::decode(Some(&body)).unwrap().plan(1),
            Err(ReservationError::Exhausted)
        ));
    }
}
