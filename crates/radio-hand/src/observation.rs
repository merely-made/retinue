//! Bounded, metadata-only radio observations.
//!
//! The wire format is deliberately independent of the recorder and carriers.  An
//! event has a per-boot sequence; a gap is a separate record class and therefore
//! never consumes an event sequence number.
//!
//! Every record is `magic:u8, version:u8, class:u8, length:u16` in big-endian,
//! followed by the class body and a big-endian CRC-32/IEEE over the header and
//! body. `length` is the complete record length, so trailing bytes are rejected.
//! The checksum detects corruption; it does not authenticate a board or carrier.
//! Unknown event kinds use all remaining body bytes as opaque payload.

pub mod collection;
pub mod owner;
pub mod recorder;

pub const MAX_RECORD_BYTES: usize = 64;
pub const UNKNOWN_DATA_MAX: usize = 30;
const MAGIC: u8 = 0x4f;
const VERSION: u8 = 1;
const EVENT_CLASS: u8 = 0;
const GAP_CLASS: u8 = 1;
const HEADER_BYTES: usize = 5;
const CHECKSUM_BYTES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationEvent {
    pub boot_id: u64,
    pub sequence: u64,
    pub uptime_ms: u64,
    pub kind: ObservationKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationGap {
    pub boot_id: u64,
    pub first_missing: u64,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationRecord {
    Event(ObservationEvent),
    Gap(ObservationGap),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    ListeningStarted {
        assignment: u16,
        profile: u8,
    },
    ListeningStopped {
        assignment: u16,
        reason: StopReason,
    },
    RxCaptured {
        profile: u8,
        length: u16,
        rssi_dbm: i16,
        snr_tenths_db: i16,
        capture_tag: u32,
    },
    RxDamaged {
        profile: u8,
    },
    TxStarted {
        profile: u8,
        length: u16,
        work: u32,
    },
    TxFinished {
        work: u32,
        outcome: TxOutcome,
    },
    WorkRefused {
        request: RequestKind,
        reason: RefusalReason,
        work: u32,
    },
    QuietStarted {
        cause: QuietCause,
    },
    QuietStopped {
        cause: QuietCause,
    },
    SleepStarted,
    SleepStopped {
        cause: WakeCause,
    },
    /// The owner can no longer certify the previous radio state. This is not
    /// a hardware stop timestamp; causes are owner-defined and preserved raw.
    ContinuityLost {
        cause: u8,
    },
    Unknown {
        kind: u8,
        data: [u8; UNKNOWN_DATA_MAX],
        len: u8,
    },
}

macro_rules! raw_enum {
    ($name:ident { $($variant:ident = $value:expr),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name { $($variant,)+ Unknown(u8) }
        impl $name {
            fn byte(self) -> u8 { match self { $(Self::$variant => $value,)+ Self::Unknown(v) => v } }
            fn from_byte(v: u8) -> Self { match v { $($value => Self::$variant,)+ _ => Self::Unknown(v) } }
        }
    };
}

raw_enum!(StopReason { Completed = 0, Reconfigured = 1, Fault = 2, Power = 3 });
raw_enum!(TxOutcome { Sent = 0, Failed = 1, Cancelled = 2 });
raw_enum!(RequestKind { Transmit = 0, Retune = 1, Listen = 2, Sleep = 3 });
raw_enum!(RefusalReason { MissingRegion = 0, DutyBudget = 1, ChannelBusy = 2, InvalidProfile = 3, ConflictingLease = 4, RadioFault = 5, QuietWindow = 6, PowerPolicy = 7 });
raw_enum!(QuietCause { Configuration = 0, Settings = 1, Announce = 2, Recovery = 3, ProfileTransition = 4, Power = 5 });
raw_enum!(WakeCause { Timer = 0, Host = 1, Radio = 2, Power = 3 });

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    BufferTooSmall,
    InvalidIdentity,
    InvalidRange,
    UnknownPayloadTooLong,
    LengthOverflow,
    InvalidKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    BadVersion,
    BadClass,
    BadLength,
    BadChecksum,
    InvalidIdentity,
    InvalidSequence,
    InvalidRange,
    InvalidKind,
    InvalidPayload,
}

impl ObservationRecord {
    pub fn encode(self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let (class, body_len) = match self {
            Self::Event(event) => {
                if event.boot_id == 0 {
                    return Err(EncodeError::InvalidIdentity);
                }
                if event.sequence == 0 {
                    return Err(EncodeError::InvalidRange);
                }
                (EVENT_CLASS, 24 + event.kind.payload_len()?)
            }
            Self::Gap(gap) => {
                if gap.boot_id == 0 {
                    return Err(EncodeError::InvalidIdentity);
                }
                if gap.first_missing == 0
                    || gap.count == 0
                    || gap.first_missing.checked_add(gap.count - 1).is_none()
                {
                    return Err(EncodeError::InvalidRange);
                }
                (GAP_CLASS, 24)
            }
        };
        let total = HEADER_BYTES
            .checked_add(body_len)
            .and_then(|n| n.checked_add(CHECKSUM_BYTES))
            .ok_or(EncodeError::LengthOverflow)?;
        if total > MAX_RECORD_BYTES {
            return Err(EncodeError::LengthOverflow);
        }
        if out.len() < total {
            return Err(EncodeError::BufferTooSmall);
        }
        out[0] = MAGIC;
        out[1] = VERSION;
        out[2] = class;
        out[3..5].copy_from_slice(&(total as u16).to_be_bytes());
        let mut p = HEADER_BYTES;
        match self {
            Self::Event(e) => {
                put_u64(out, &mut p, e.boot_id);
                put_u64(out, &mut p, e.sequence);
                put_u64(out, &mut p, e.uptime_ms);
                e.kind.encode_payload(out, &mut p)?;
            }
            Self::Gap(g) => {
                put_u64(out, &mut p, g.boot_id);
                put_u64(out, &mut p, g.first_missing);
                put_u64(out, &mut p, g.count);
            }
        }
        let crc = crc32(&out[..total - CHECKSUM_BYTES]);
        out[total - 4..total].copy_from_slice(&crc.to_be_bytes());
        Ok(total)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES {
            return Err(DecodeError::Truncated);
        }
        if bytes[0] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if bytes[1] != VERSION {
            return Err(DecodeError::BadVersion);
        }
        let total = u16::from_be_bytes([bytes[3], bytes[4]]) as usize;
        if !(HEADER_BYTES + CHECKSUM_BYTES..=MAX_RECORD_BYTES).contains(&total) {
            return Err(DecodeError::BadLength);
        }
        if bytes.len() < total {
            return Err(DecodeError::Truncated);
        }
        if bytes.len() != total {
            return Err(DecodeError::BadLength);
        }
        let expected = u32::from_be_bytes(bytes[total - 4..total].try_into().unwrap());
        if crc32(&bytes[..total - 4]) != expected {
            return Err(DecodeError::BadChecksum);
        }
        let mut p = HEADER_BYTES;
        let record = match bytes[2] {
            EVENT_CLASS => {
                if total < HEADER_BYTES + CHECKSUM_BYTES + 24 {
                    return Err(DecodeError::BadLength);
                }
                let e = ObservationEvent {
                    boot_id: get_u64(bytes, &mut p),
                    sequence: get_u64(bytes, &mut p),
                    uptime_ms: get_u64(bytes, &mut p),
                    kind: ObservationKind::decode_payload(bytes, &mut p, total - 4)?,
                };
                if e.boot_id == 0 {
                    return Err(DecodeError::InvalidIdentity);
                }
                if e.sequence == 0 {
                    return Err(DecodeError::InvalidSequence);
                }
                Self::Event(e)
            }
            GAP_CLASS => {
                if total != HEADER_BYTES + 24 + CHECKSUM_BYTES {
                    return Err(DecodeError::BadLength);
                }
                let g = ObservationGap {
                    boot_id: get_u64(bytes, &mut p),
                    first_missing: get_u64(bytes, &mut p),
                    count: get_u64(bytes, &mut p),
                };
                if g.boot_id == 0 {
                    return Err(DecodeError::InvalidIdentity);
                }
                if g.first_missing == 0
                    || g.count == 0
                    || g.first_missing.checked_add(g.count - 1).is_none()
                {
                    return Err(DecodeError::InvalidRange);
                }
                Self::Gap(g)
            }
            _ => return Err(DecodeError::BadClass),
        };
        Ok(record)
    }
}

impl ObservationKind {
    fn payload_len(self) -> Result<usize, EncodeError> {
        Ok(match self {
            Self::ListeningStarted { .. } => 1 + 2 + 1,
            Self::ListeningStopped { .. } => 1 + 2 + 1,
            Self::RxCaptured { .. } => 1 + 1 + 2 + 2 + 2 + 4,
            Self::RxDamaged { .. } => 2,
            Self::TxStarted { .. } => 1 + 1 + 2 + 4,
            Self::TxFinished { .. } => 1 + 4 + 1,
            Self::WorkRefused { .. } => 1 + 1 + 1 + 4,
            Self::QuietStarted { .. }
            | Self::QuietStopped { .. }
            | Self::SleepStopped { .. }
            | Self::ContinuityLost { .. } => 2,
            Self::SleepStarted => 1,
            Self::Unknown { len, .. } => {
                if usize::from(len) > UNKNOWN_DATA_MAX {
                    return Err(EncodeError::UnknownPayloadTooLong);
                }
                1 + usize::from(len)
            }
        })
    }
    fn encode_payload(self, out: &mut [u8], p: &mut usize) -> Result<(), EncodeError> {
        let tag = match self {
            Self::ListeningStarted { .. } => 0,
            Self::ListeningStopped { .. } => 1,
            Self::RxCaptured { .. } => 2,
            Self::RxDamaged { .. } => 3,
            Self::TxStarted { .. } => 4,
            Self::TxFinished { .. } => 5,
            Self::WorkRefused { .. } => 6,
            Self::QuietStarted { .. } => 7,
            Self::QuietStopped { .. } => 8,
            Self::SleepStarted => 9,
            Self::SleepStopped { .. } => 10,
            Self::ContinuityLost { .. } => 11,
            Self::Unknown { kind, .. } => kind,
        };
        out[*p] = tag;
        *p += 1;
        match self {
            Self::ListeningStarted {
                assignment,
                profile,
            } => {
                put_u16(out, p, assignment);
                out[*p] = profile;
                *p += 1
            }
            Self::ListeningStopped { assignment, reason } => {
                put_u16(out, p, assignment);
                out[*p] = reason.byte();
                *p += 1
            }
            Self::RxCaptured {
                profile,
                length,
                rssi_dbm,
                snr_tenths_db,
                capture_tag,
            } => {
                out[*p] = profile;
                *p += 1;
                put_u16(out, p, length);
                put_i16(out, p, rssi_dbm);
                put_i16(out, p, snr_tenths_db);
                put_u32(out, p, capture_tag)
            }
            Self::RxDamaged { profile } => {
                out[*p] = profile;
                *p += 1
            }
            Self::TxStarted {
                profile,
                length,
                work,
            } => {
                out[*p] = profile;
                *p += 1;
                put_u16(out, p, length);
                put_u32(out, p, work)
            }
            Self::TxFinished { work, outcome } => {
                put_u32(out, p, work);
                out[*p] = outcome.byte();
                *p += 1
            }
            Self::WorkRefused {
                request,
                reason,
                work,
            } => {
                out[*p] = request.byte();
                out[*p + 1] = reason.byte();
                *p += 2;
                put_u32(out, p, work)
            }
            Self::QuietStarted { cause } | Self::QuietStopped { cause } => {
                out[*p] = cause.byte();
                *p += 1
            }
            Self::SleepStarted => {}
            Self::SleepStopped { cause } => {
                out[*p] = cause.byte();
                *p += 1
            }
            Self::ContinuityLost { cause } => {
                out[*p] = cause;
                *p += 1;
            }
            Self::Unknown { kind, data, len } => {
                if kind <= 11 {
                    return Err(EncodeError::InvalidKind);
                }
                out[*p..*p + usize::from(len)].copy_from_slice(&data[..usize::from(len)]);
                *p += usize::from(len);
            }
        };
        Ok(())
    }
    fn decode_payload(b: &[u8], p: &mut usize, end: usize) -> Result<Self, DecodeError> {
        if *p >= end {
            return Err(DecodeError::InvalidPayload);
        }
        let tag = b[*p];
        *p += 1;
        let need = |p: &mut usize, n: usize| {
            if p.checked_add(n).is_none_or(|v| v > end) {
                Err(DecodeError::InvalidPayload)
            } else {
                Ok(())
            }
        };
        let v = match tag {
            0 => {
                need(p, 3)?;
                let a = get_u16(b, p);
                let profile = b[*p];
                *p += 1;
                Self::ListeningStarted {
                    assignment: a,
                    profile,
                }
            }
            1 => {
                need(p, 3)?;
                let a = get_u16(b, p);
                let r = StopReason::from_byte(b[*p]);
                *p += 1;
                Self::ListeningStopped {
                    assignment: a,
                    reason: r,
                }
            }
            2 => {
                need(p, 11)?;
                let profile = b[*p];
                *p += 1;
                let length = get_u16(b, p);
                let r = get_i16(b, p);
                let s = get_i16(b, p);
                let t = get_u32(b, p);
                Self::RxCaptured {
                    profile,
                    length,
                    rssi_dbm: r,
                    snr_tenths_db: s,
                    capture_tag: t,
                }
            }
            3 => {
                need(p, 1)?;
                let profile = b[*p];
                *p += 1;
                Self::RxDamaged { profile }
            }
            4 => {
                need(p, 7)?;
                let profile = b[*p];
                *p += 1;
                let length = get_u16(b, p);
                let work = get_u32(b, p);
                Self::TxStarted {
                    profile,
                    length,
                    work,
                }
            }
            5 => {
                need(p, 5)?;
                let work = get_u32(b, p);
                let outcome = TxOutcome::from_byte(b[*p]);
                *p += 1;
                Self::TxFinished { work, outcome }
            }
            6 => {
                need(p, 6)?;
                let request = RequestKind::from_byte(b[*p]);
                let reason = RefusalReason::from_byte(b[*p + 1]);
                *p += 2;
                let work = get_u32(b, p);
                Self::WorkRefused {
                    request,
                    reason,
                    work,
                }
            }
            7 => {
                need(p, 1)?;
                let cause = QuietCause::from_byte(b[*p]);
                *p += 1;
                Self::QuietStarted { cause }
            }
            8 => {
                need(p, 1)?;
                let cause = QuietCause::from_byte(b[*p]);
                *p += 1;
                Self::QuietStopped { cause }
            }
            9 => Self::SleepStarted,
            10 => {
                need(p, 1)?;
                let cause = WakeCause::from_byte(b[*p]);
                *p += 1;
                Self::SleepStopped { cause }
            }
            11 => {
                need(p, 1)?;
                let cause = b[*p];
                *p += 1;
                Self::ContinuityLost { cause }
            }
            _ => {
                let len = end - *p;
                if len > UNKNOWN_DATA_MAX {
                    return Err(DecodeError::InvalidPayload);
                }
                let mut data = [0u8; UNKNOWN_DATA_MAX];
                data[..len].copy_from_slice(&b[*p..*p + len]);
                *p += len;
                Self::Unknown {
                    kind: tag,
                    data,
                    len: len as u8,
                }
            }
        };
        if *p != end && tag <= 11 {
            return Err(DecodeError::InvalidPayload);
        }
        Ok(v)
    }
}

fn put_u16(b: &mut [u8], p: &mut usize, v: u16) {
    b[*p..*p + 2].copy_from_slice(&v.to_be_bytes());
    *p += 2
}
fn put_u32(b: &mut [u8], p: &mut usize, v: u32) {
    b[*p..*p + 4].copy_from_slice(&v.to_be_bytes());
    *p += 4
}
fn put_u64(b: &mut [u8], p: &mut usize, v: u64) {
    b[*p..*p + 8].copy_from_slice(&v.to_be_bytes());
    *p += 8
}
fn put_i16(b: &mut [u8], p: &mut usize, v: i16) {
    put_u16(b, p, v as u16)
}
fn get_u16(b: &[u8], p: &mut usize) -> u16 {
    let v = u16::from_be_bytes([b[*p], b[*p + 1]]);
    *p += 2;
    v
}
fn get_u32(b: &[u8], p: &mut usize) -> u32 {
    let v = u32::from_be_bytes(b[*p..*p + 4].try_into().unwrap());
    *p += 4;
    v
}
fn get_u64(b: &[u8], p: &mut usize) -> u64 {
    let v = u64::from_be_bytes(b[*p..*p + 8].try_into().unwrap());
    *p += 8;
    v
}
fn get_i16(b: &[u8], p: &mut usize) -> i16 {
    get_u16(b, p) as i16
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crc_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
    #[test]
    fn all_kinds_fit_and_roundtrip() {
        let kinds = [
            ObservationKind::ListeningStarted {
                assignment: 2,
                profile: 3,
            },
            ObservationKind::ListeningStopped {
                assignment: 2,
                reason: StopReason::Fault,
            },
            ObservationKind::RxCaptured {
                profile: 3,
                length: 4,
                rssi_dbm: -70,
                snr_tenths_db: 125,
                capture_tag: 9,
            },
            ObservationKind::RxDamaged { profile: 3 },
            ObservationKind::TxStarted {
                profile: 3,
                length: 4,
                work: 9,
            },
            ObservationKind::TxFinished {
                work: 9,
                outcome: TxOutcome::Sent,
            },
            ObservationKind::WorkRefused {
                request: RequestKind::Transmit,
                reason: RefusalReason::DutyBudget,
                work: 9,
            },
            ObservationKind::QuietStarted {
                cause: QuietCause::Configuration,
            },
            ObservationKind::QuietStopped {
                cause: QuietCause::Configuration,
            },
            ObservationKind::SleepStarted,
            ObservationKind::SleepStopped {
                cause: WakeCause::Timer,
            },
        ];
        for kind in kinds {
            let r = ObservationRecord::Event(ObservationEvent {
                boot_id: 1,
                sequence: 1,
                uptime_ms: 2,
                kind,
            });
            let mut b = [0; MAX_RECORD_BYTES];
            let n = r.encode(&mut b).unwrap();
            assert_eq!(ObservationRecord::decode(&b[..n]).unwrap(), r);
        }
    }

    #[test]
    fn literal_event_fixtures_pin_every_known_kind() {
        let records = [
            (
                ObservationKind::ListeningStarted {
                    assignment: 2,
                    profile: 3,
                },
                &[
                    79, 1, 0, 0, 37, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 0, 0, 2, 3, 199, 106, 211, 144,
                ][..],
            ),
            (
                ObservationKind::ListeningStopped {
                    assignment: 2,
                    reason: StopReason::Fault,
                },
                &[
                    79, 1, 0, 0, 37, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 1, 0, 2, 2, 8, 209, 132, 99,
                ][..],
            ),
            (
                ObservationKind::RxCaptured {
                    profile: 3,
                    length: 4,
                    rssi_dbm: -70,
                    snr_tenths_db: 125,
                    capture_tag: 9,
                },
                &[
                    79, 1, 0, 0, 45, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 2, 3, 0, 4, 255, 186, 0, 125, 0, 0, 0, 9, 49, 232, 8, 113,
                ][..],
            ),
            (
                ObservationKind::RxDamaged { profile: 3 },
                &[
                    79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 3, 3, 173, 72, 89, 35,
                ][..],
            ),
            (
                ObservationKind::TxStarted {
                    profile: 3,
                    length: 4,
                    work: 9,
                },
                &[
                    79, 1, 0, 0, 41, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 4, 3, 0, 4, 0, 0, 0, 9, 13, 96, 114, 34,
                ][..],
            ),
            (
                ObservationKind::TxFinished {
                    work: 9,
                    outcome: TxOutcome::Sent,
                },
                &[
                    79, 1, 0, 0, 39, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 5, 0, 0, 0, 9, 0, 27, 205, 81, 107,
                ][..],
            ),
            (
                ObservationKind::WorkRefused {
                    request: RequestKind::Transmit,
                    reason: RefusalReason::DutyBudget,
                    work: 9,
                },
                &[
                    79, 1, 0, 0, 40, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 6, 0, 1, 0, 0, 0, 9, 41, 125, 12, 203,
                ][..],
            ),
            (
                ObservationKind::QuietStarted {
                    cause: QuietCause::Configuration,
                },
                &[
                    79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 7, 0, 80, 45, 205, 157,
                ][..],
            ),
            (
                ObservationKind::QuietStopped {
                    cause: QuietCause::Configuration,
                },
                &[
                    79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 8, 0, 215, 181, 209, 82,
                ][..],
            ),
            (
                ObservationKind::SleepStarted,
                &[
                    79, 1, 0, 0, 34, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 9, 39, 115, 90, 198,
                ][..],
            ),
            (
                ObservationKind::SleepStopped {
                    cause: WakeCause::Timer,
                },
                &[
                    79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
                    0, 0, 2, 10, 0, 229, 131, 179, 208,
                ][..],
            ),
        ];
        for (kind, expected) in records {
            let mut bytes = [0; MAX_RECORD_BYTES];
            let n = ObservationRecord::Event(ObservationEvent {
                boot_id: 1,
                sequence: 1,
                uptime_ms: 2,
                kind,
            })
            .encode(&mut bytes)
            .unwrap();
            assert_eq!(&bytes[..n], expected);
            assert_eq!(
                ObservationRecord::decode(expected),
                Ok(ObservationRecord::Event(ObservationEvent {
                    boot_id: 1,
                    sequence: 1,
                    uptime_ms: 2,
                    kind
                }))
            );
        }
    }

    #[test]
    fn boundary_unknown_and_raw_reasons_are_literal_and_lossless() {
        let mut data = [0u8; UNKNOWN_DATA_MAX];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = i as u8;
        }
        let record = ObservationRecord::Event(ObservationEvent {
            boot_id: 9,
            sequence: 7,
            uptime_ms: 6,
            kind: ObservationKind::Unknown {
                kind: 200,
                data,
                len: UNKNOWN_DATA_MAX as u8,
            },
        });
        let mut bytes = [0u8; MAX_RECORD_BYTES];
        let n = record.encode(&mut bytes).unwrap();
        let expected = [
            79, 1, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0,
            6, 200, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
            22, 23, 24, 25, 26, 27, 28, 29, 235, 153, 238, 180,
        ];
        assert_eq!(&bytes[..n], &expected);
        assert_eq!(n, MAX_RECORD_BYTES);
        assert_eq!(ObservationRecord::decode(&bytes[..n]), Ok(record));
        let raw = ObservationRecord::Event(ObservationEvent {
            boot_id: 1,
            sequence: 1,
            uptime_ms: 1,
            kind: ObservationKind::ListeningStopped {
                assignment: 1,
                reason: StopReason::Unknown(99),
            },
        });
        let n = raw.encode(&mut bytes).unwrap();
        assert_eq!(ObservationRecord::decode(&bytes[..n]), Ok(raw));
    }

    #[test]
    fn every_truncation_point_and_checksum_valid_payload_mutation_reject() {
        let record = ObservationRecord::Event(ObservationEvent {
            boot_id: 1,
            sequence: 1,
            uptime_ms: 0,
            kind: ObservationKind::SleepStarted,
        });
        let mut bytes = [0u8; MAX_RECORD_BYTES];
        let n = record.encode(&mut bytes).unwrap();
        for end in 0..n {
            assert!(
                ObservationRecord::decode(&bytes[..end]).is_err(),
                "accepted truncation at {end}"
            );
        }
        let mut malformed = [0u8; MAX_RECORD_BYTES];
        malformed[..n].copy_from_slice(&bytes[..n]);
        malformed[n - 4] = 0; // append one payload byte before the checksum
        malformed[4] += 1;
        let crc = crc32(&malformed[..n - 3]);
        malformed[n - 3..n + 1].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            ObservationRecord::decode(&malformed[..n + 1]),
            Err(DecodeError::InvalidPayload)
        );
    }

    #[test]
    fn zero_identity_sequence_and_gap_end_overflow_are_rejected_after_checksum() {
        let record = ObservationRecord::Event(ObservationEvent {
            boot_id: 1,
            sequence: 1,
            uptime_ms: 0,
            kind: ObservationKind::SleepStarted,
        });
        let mut bytes = [0u8; MAX_RECORD_BYTES];
        let n = record.encode(&mut bytes).unwrap();
        bytes[5..13].fill(0);
        let crc = crc32(&bytes[..n - 4]);
        bytes[n - 4..n].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            ObservationRecord::decode(&bytes[..n]),
            Err(DecodeError::InvalidIdentity)
        );
        let n = record.encode(&mut bytes).unwrap();
        bytes[13..21].fill(0);
        let crc = crc32(&bytes[..n - 4]);
        bytes[n - 4..n].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            ObservationRecord::decode(&bytes[..n]),
            Err(DecodeError::InvalidSequence)
        );
        assert_eq!(
            ObservationRecord::Gap(ObservationGap {
                boot_id: 1,
                first_missing: u64::MAX,
                count: 2
            })
            .encode(&mut bytes),
            Err(EncodeError::InvalidRange)
        );
    }
    #[test]
    fn unknown_and_gap_roundtrip() {
        let mut data = [0; UNKNOWN_DATA_MAX];
        data[..3].copy_from_slice(&[1, 2, 3]);
        let r = ObservationRecord::Event(ObservationEvent {
            boot_id: 2,
            sequence: 3,
            uptime_ms: 4,
            kind: ObservationKind::Unknown {
                kind: 99,
                data,
                len: 3,
            },
        });
        let mut b = [0; MAX_RECORD_BYTES];
        let n = r.encode(&mut b).unwrap();
        assert_eq!(ObservationRecord::decode(&b[..n]).unwrap(), r);
        let g = ObservationRecord::Gap(ObservationGap {
            boot_id: 2,
            first_missing: 8,
            count: 4,
        });
        let n = g.encode(&mut b).unwrap();
        let expected = [
            79, 1, 1, 0, 33, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0,
            4, 51, 74, 249, 166,
        ];
        assert_eq!(&b[..n], &expected);
        assert_eq!(ObservationRecord::decode(&b[..n]).unwrap(), g);
    }

    #[test]
    fn continuity_loss_has_an_independent_literal() {
        // Python struct.pack big-endian fields and zlib.crc32, independent of
        // this encoder. A loss marker asserts neither standby nor its time.
        let literal = [
            79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0,
            6, 11, 3, 131, 140, 172, 49,
        ];
        let record = ObservationRecord::Event(ObservationEvent {
            boot_id: 9,
            sequence: 7,
            uptime_ms: 6,
            kind: ObservationKind::ContinuityLost { cause: 3 },
        });
        assert_eq!(ObservationRecord::decode(&literal), Ok(record));
        let mut bytes = [0; MAX_RECORD_BYTES];
        let len = record.encode(&mut bytes).unwrap();
        assert_eq!(&bytes[..len], &literal);
    }
    #[test]
    fn unknown_empty_payload_is_preserved() {
        let record = ObservationRecord::Event(ObservationEvent {
            boot_id: 1,
            sequence: u64::MAX,
            uptime_ms: 0,
            kind: ObservationKind::Unknown {
                kind: 200,
                data: [0; UNKNOWN_DATA_MAX],
                len: 0,
            },
        });
        let mut bytes = [0; MAX_RECORD_BYTES];
        let len = record.encode(&mut bytes).unwrap();
        assert_eq!(len, 34);
        assert_eq!(ObservationRecord::decode(&bytes[..len]), Ok(record));
    }

    #[test]
    fn rejects_trailing_truncated_and_bad_values() {
        let r = ObservationRecord::Event(ObservationEvent {
            boot_id: 1,
            sequence: 1,
            uptime_ms: 0,
            kind: ObservationKind::SleepStarted,
        });
        let mut b = [0; MAX_RECORD_BYTES];
        let n = r.encode(&mut b).unwrap();
        assert_eq!(
            ObservationRecord::decode(&b[..n - 1]),
            Err(DecodeError::Truncated)
        );
        assert_eq!(
            ObservationRecord::decode(&b[..n + 1]),
            Err(DecodeError::BadLength)
        );
        b[4] = 0;
        assert_eq!(
            ObservationRecord::decode(&b[..n]),
            Err(DecodeError::BadLength)
        );
    }
}
