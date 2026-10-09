//! Record encoding and decoding: framing, per-kind payloads, and the CRC-32 trailer.

use super::*;

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
pub(super) fn crc32(bytes: &[u8]) -> u32 {
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
