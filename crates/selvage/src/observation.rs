//! Bounded, allocation-free direct-PHY observation transport.

pub const CMD_OBSERVATION: u8 = 0x04;
pub const EVENT_OBSERVATION: u8 = 0x86;
pub const OBSERVATION_VERSION: u8 = 1;
pub const MAX_OBSERVATION_RECORD_LEN: usize = 64;
pub const OBSERVATION_COMMAND_BODY_LEN: usize = 45;
pub const MAX_OBSERVATION_COMMAND_BODY_LEN: usize = OBSERVATION_COMMAND_BODY_LEN;
pub const MAX_OBSERVATION_COMMAND_LEN: usize = OBSERVATION_COMMAND_BODY_LEN + 1;
pub const MAX_OBSERVATION_REPLY_PAYLOAD_LEN: usize = 129;
pub const MAX_OBSERVATION_REPLY_LEN: usize = 1 + 2 + MAX_OBSERVATION_REPLY_PAYLOAD_LEN + 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    Cursor {
        request_id: u32,
        boot_id: u64,
        after_sequence: u64,
    },
    Profile {
        request_id: u32,
        boot_id: u64,
        profile_id: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Ok,
    Disabled,
    WrongBoot,
    FutureCursor,
    InvalidRequest,
    UnknownProfile,
}

impl Status {
    const fn wire(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Disabled => 1,
            Self::WrongBoot => 2,
            Self::FutureCursor => 3,
            Self::InvalidRequest => 4,
            Self::UnknownProfile => 5,
        }
    }
    const fn from_wire(value: u8) -> Result<Self, WireError> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::Disabled),
            2 => Ok(Self::WrongBoot),
            3 => Ok(Self::FutureCursor),
            4 => Ok(Self::InvalidRequest),
            5 => Ok(Self::UnknownProfile),
            _ => Err(WireError::InvalidStatus),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorReply {
    pub request_id: u32,
    pub status: Status,
    pub boot_id: u64,
    pub oldest: u64,
    pub newest: u64,
    pub next: u64,
    pub recorded: u64,
    pub overwritten: u64,
    pub encode_failed: u64,
    pub profile_count: u8,
    pub record_len: u8,
    pub record: [u8; MAX_OBSERVATION_RECORD_LEN],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileReply {
    pub request_id: u32,
    pub status: Status,
    pub boot_id: u64,
    pub profile_id: u8,
    pub config: [u8; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reply {
    Cursor(CursorReply),
    Profile(ProfileReply),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    InvalidMarker,
    InvalidLength,
    InvalidHex,
    InvalidVersion,
    InvalidKind,
    InvalidStatus,
    InvalidRecordLength,
    InvalidRequest,
    InvalidCrc,
}

pub fn encode_request(request: Request, output: &mut [u8; MAX_OBSERVATION_COMMAND_LEN]) -> usize {
    let (kind, request_id, boot_id, argument) = match request {
        Request::Cursor {
            request_id,
            boot_id,
            after_sequence,
        } => (0, request_id, boot_id, after_sequence),
        Request::Profile {
            request_id,
            boot_id,
            profile_id,
        } => (1, request_id, boot_id, profile_id as u64),
    };
    let mut raw = [0u8; 22];
    raw[0] = OBSERVATION_VERSION;
    raw[1] = kind;
    raw[2..6].copy_from_slice(&request_id.to_be_bytes());
    raw[6..14].copy_from_slice(&boot_id.to_be_bytes());
    raw[14..22].copy_from_slice(&argument.to_be_bytes());
    output[0] = CMD_OBSERVATION;
    for (i, byte) in raw.iter().copied().enumerate() {
        output[1 + i * 2] = hex(byte >> 4);
        output[2 + i * 2] = hex(byte & 0xf);
    }
    output[45] = 0;
    MAX_OBSERVATION_COMMAND_LEN
}

pub fn decode_request(body: &[u8]) -> Result<Request, WireError> {
    if body.len() != OBSERVATION_COMMAND_BODY_LEN || body[0] != CMD_OBSERVATION {
        return Err(WireError::InvalidLength);
    }
    let mut raw = [0u8; 22];
    for i in 0..22 {
        raw[i] = (unhex(body[1 + i * 2])? << 4) | unhex(body[2 + i * 2])?;
    }
    if raw[0] != OBSERVATION_VERSION {
        return Err(WireError::InvalidVersion);
    }
    let kind = raw[1];
    let request_id = u32::from_be_bytes(raw[2..6].try_into().unwrap());
    let boot_id = u64::from_be_bytes(raw[6..14].try_into().unwrap());
    let argument = u64::from_be_bytes(raw[14..22].try_into().unwrap());
    match kind {
        0 if boot_id != 0 || argument == 0 => Ok(Request::Cursor {
            request_id,
            boot_id,
            after_sequence: argument,
        }),
        0 => Err(WireError::InvalidRequest),
        1 if boot_id != 0 && argument <= u8::MAX as u64 => Ok(Request::Profile {
            request_id,
            boot_id,
            profile_id: argument as u8,
        }),
        1 => Err(WireError::InvalidRequest),
        _ => Err(WireError::InvalidKind),
    }
}

pub fn encode_reply(
    reply: Reply,
    output: &mut [u8; MAX_OBSERVATION_REPLY_LEN],
) -> Result<usize, WireError> {
    let mut payload = [0u8; MAX_OBSERVATION_REPLY_PAYLOAD_LEN];
    let payload_len = match reply {
        Reply::Cursor(r) => {
            if r.record_len as usize > MAX_OBSERVATION_RECORD_LEN
                || (r.status != Status::Ok && r.record_len != 0)
            {
                return Err(WireError::InvalidRecordLength);
            }
            payload[1] = 0;
            write_common(&mut payload, r.request_id, r.status, r.boot_id);
            for (at, value) in [
                r.oldest,
                r.newest,
                r.next,
                r.recorded,
                r.overwritten,
                r.encode_failed,
            ]
            .iter()
            .copied()
            .enumerate()
            {
                payload[15 + at * 8..23 + at * 8].copy_from_slice(&value.to_be_bytes());
            }
            payload[63] = r.profile_count;
            payload[64] = r.record_len;
            payload[65..65 + r.record_len as usize]
                .copy_from_slice(&r.record[..r.record_len as usize]);
            65 + r.record_len as usize
        }
        Reply::Profile(r) => {
            payload[1] = 1;
            write_common(&mut payload, r.request_id, r.status, r.boot_id);
            payload[15] = r.profile_id;
            payload[16..32].copy_from_slice(&r.config);
            32
        }
    };
    output[0] = EVENT_OBSERVATION;
    output[1..3].copy_from_slice(&(payload_len as u16).to_be_bytes());
    output[3..3 + payload_len].copy_from_slice(&payload[..payload_len]);
    let crc = crc32(&output[..3 + payload_len]);
    output[3 + payload_len..7 + payload_len].copy_from_slice(&crc.to_be_bytes());
    Ok(7 + payload_len)
}

pub fn decode_reply(frame: &[u8]) -> Result<Reply, WireError> {
    if frame.len() < 7 || frame[0] != EVENT_OBSERVATION {
        return Err(WireError::InvalidMarker);
    }
    let payload_len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
    if payload_len > MAX_OBSERVATION_REPLY_PAYLOAD_LEN || frame.len() != 7 + payload_len {
        return Err(WireError::InvalidLength);
    }
    let expected = u32::from_be_bytes(frame[3 + payload_len..7 + payload_len].try_into().unwrap());
    if crc32(&frame[..3 + payload_len]) != expected {
        return Err(WireError::InvalidCrc);
    }
    let p = &frame[3..3 + payload_len];
    if p.len() < 15 {
        return Err(WireError::InvalidLength);
    }
    let version = p[0];
    if version != OBSERVATION_VERSION {
        return Err(WireError::InvalidVersion);
    }
    let request_id = u32::from_be_bytes(p[2..6].try_into().unwrap());
    let status = Status::from_wire(p[6])?;
    let boot_id = u64::from_be_bytes(p[7..15].try_into().unwrap());
    match p[1] {
        0 => {
            if p.len() < 65 {
                return Err(WireError::InvalidLength);
            }
            let record_len = p[64] as usize;
            if record_len > 64
                || p.len() != 65 + record_len
                || (status != Status::Ok && record_len != 0)
            {
                return Err(WireError::InvalidRecordLength);
            }
            let mut record = [0u8; 64];
            record[..record_len].copy_from_slice(&p[65..]);
            let mut nums = [0u64; 6];
            for (i, n) in nums.iter_mut().enumerate() {
                *n = u64::from_be_bytes(p[15 + i * 8..23 + i * 8].try_into().unwrap());
            }
            Ok(Reply::Cursor(CursorReply {
                request_id,
                status,
                boot_id,
                oldest: nums[0],
                newest: nums[1],
                next: nums[2],
                recorded: nums[3],
                overwritten: nums[4],
                encode_failed: nums[5],
                profile_count: p[63],
                record_len: record_len as u8,
                record,
            }))
        }
        1 => {
            if p.len() != 32 {
                return Err(WireError::InvalidLength);
            }
            let mut config = [0u8; 16];
            config.copy_from_slice(&p[16..]);
            Ok(Reply::Profile(ProfileReply {
                request_id,
                status,
                boot_id,
                profile_id: p[15],
                config,
            }))
        }
        _ => Err(WireError::InvalidKind),
    }
}

fn write_common(p: &mut [u8], request_id: u32, status: Status, boot_id: u64) {
    p[0] = OBSERVATION_VERSION;
    p[2..6].copy_from_slice(&request_id.to_be_bytes());
    p[6] = status.wire();
    p[7..15].copy_from_slice(&boot_id.to_be_bytes());
}
const fn hex(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        _ => b'a' + n - 10,
    }
}
const fn unhex(n: u8) -> Result<u8, WireError> {
    match n {
        b'0'..=b'9' => Ok(n - b'0'),
        b'a'..=b'f' => Ok(n - b'a' + 10),
        _ => Err(WireError::InvalidHex),
    }
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for &byte in bytes {
        crc ^= byte as u32;
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
    fn request_round_trip_and_literal() {
        let r = Request::Cursor {
            request_id: 0x0102_0304,
            boot_id: 0x0506_0708_090a_0b0c,
            after_sequence: 0,
        };
        let mut b = [0; MAX_OBSERVATION_COMMAND_LEN];
        encode_request(r, &mut b);
        assert_eq!(&b[..5], &[0x04, b'0', b'1', b'0', b'0']);
        assert_eq!(decode_request(&b[..45]), Ok(r));
    }
    #[test]
    fn profile_round_trip() {
        let r = Request::Profile {
            request_id: 7,
            boot_id: 9,
            profile_id: 3,
        };
        let mut b = [0; MAX_OBSERVATION_COMMAND_LEN];
        encode_request(r, &mut b);
        assert_eq!(decode_request(&b[..45]), Ok(r));
    }
    #[test]
    fn reply_empty_and_crc() {
        let r = Reply::Cursor(CursorReply {
            request_id: 1,
            status: Status::Ok,
            boot_id: 2,
            oldest: 0,
            newest: 0,
            next: 0,
            recorded: 0,
            overwritten: 0,
            encode_failed: 0,
            profile_count: 0,
            record_len: 0,
            record: [0; 64],
        });
        let mut b = [0; MAX_OBSERVATION_REPLY_LEN];
        let n = encode_reply(r, &mut b).unwrap();
        assert_eq!(n, 72);
        assert_eq!(decode_reply(&b[..n]), Ok(r));
        let mut bad = b;
        bad[10] ^= 1;
        assert_eq!(decode_reply(&bad[..n]), Err(WireError::InvalidCrc));
    }
    #[test]
    fn independently_literal_python_vectors() {
        let request = [
            0x04, b'0', b'1', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0',
            b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0',
            b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0',
            b'0', b'0', b'0', b'0', b'0', b'0', 0,
        ];
        assert_eq!(
            decode_request(&request[..45]),
            Ok(Request::Cursor {
                request_id: 0,
                boot_id: 0,
                after_sequence: 0
            })
        );
        let profile = literal_hex::<39>(
            "86002001010000000700000000000000000903000102030405060708090a0b0c0d0e0f1b8d45b3",
        );
        assert_eq!(
            decode_reply(&profile),
            Ok(Reply::Profile(ProfileReply {
                request_id: 7,
                status: Status::Ok,
                boot_id: 9,
                profile_id: 3,
                config: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
            }))
        );
    }
    fn literal_hex<const N: usize>(text: &str) -> [u8; N] {
        let bytes = text.as_bytes();
        let mut out = [0; N];
        for i in 0..N {
            out[i] = (digit(bytes[i * 2]) << 4) | digit(bytes[i * 2 + 1]);
        }
        out
    }
    fn digit(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => 255,
        }
    }
    #[test]
    fn rejects_truncated_oversize_and_bad_profile() {
        let mut b = [0; MAX_OBSERVATION_COMMAND_LEN];
        encode_request(
            Request::Cursor {
                request_id: 0,
                boot_id: 0,
                after_sequence: 0,
            },
            &mut b,
        );
        assert!(decode_request(&b[..44]).is_err());
        assert!(decode_request(&b[..45]).is_ok());
        let mut p = b;
        p[4] = b'1';
        assert_eq!(decode_request(&p[..45]), Err(WireError::InvalidRequest));
    }
}
