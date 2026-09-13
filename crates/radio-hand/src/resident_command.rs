//! Commands for a configured resident runtime only. Wire: `08 len:u16-LE body`.
//! Body begins with opcode: 0 status, 1 excursion, 2 cancel, 3 Sennet text,
//! 4 Tucket text, 5 Tucket advert. Multibyte integers are little-endian.
//! Fixed lengths and UTF-8 are validated before constructing owned values.
use heapless::{String, Vec};
pub const MARKER: u8 = 8;
pub const MAX_BODY: usize = 255;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Excursion {
        target: u8,
        duration_ms: u64,
        allow_loss: bool,
    },
    Cancel,
    SennetText {
        destination: u32,
        hops: u8,
        want_ack: bool,
        text: String<232>,
    },
    TucketText {
        to: u8,
        timestamp: u32,
        ttl_ms: u64,
        attempts: u8,
        flood_last: bool,
        text: String<171>,
    },
    TucketAdvert {
        timestamp: u32,
        data: Vec<u8, 32>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
// Fixed inline payload is deliberate: the command parser is allocation-free.
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Pending,
    Complete(Command),
    Rejected,
}
impl Command {
    pub fn decode(body: &[u8]) -> Option<Self> {
        if body.len() > MAX_BODY {
            return None;
        }
        Some(match *body.first()? {
            0 if body.len() == 1 => Self::Status,
            1 if body.len() == 11 && body[1] < 3 && body[10] <= 1 => {
                let duration_ms = u64::from_le_bytes(body[2..10].try_into().ok()?);
                if duration_ms == 0 {
                    return None;
                }
                Self::Excursion {
                    target: body[1],
                    duration_ms,
                    allow_loss: body[10] == 1,
                }
            }
            2 if body.len() == 1 => Self::Cancel,
            3 if body.len() >= 7 && body[5] <= 7 && body[6] <= 1 => Self::SennetText {
                destination: u32::from_le_bytes(body[1..5].try_into().ok()?),
                hops: body[5],
                want_ack: body[6] == 1,
                text: String::try_from(core::str::from_utf8(&body[7..]).ok()?).ok()?,
            },
            4 if body.len() >= 16 && (1..=4).contains(&body[14]) && body[15] <= 1 => {
                let ttl_ms = u64::from_le_bytes(body[6..14].try_into().ok()?);
                if ttl_ms == 0 {
                    return None;
                }
                Self::TucketText {
                    to: body[1],
                    timestamp: u32::from_le_bytes(body[2..6].try_into().ok()?),
                    ttl_ms,
                    attempts: body[14],
                    flood_last: body[15] == 1,
                    text: String::try_from(core::str::from_utf8(&body[16..]).ok()?).ok()?,
                }
            }
            5 if body.len() >= 5 => Self::TucketAdvert {
                timestamp: u32::from_le_bytes(body[1..5].try_into().ok()?),
                data: Vec::from_slice(&body[5..]).ok()?,
            },
            _ => return None,
        })
    }
    /// Complete framed command. Returns None for invalid public field values.
    pub fn encode(&self) -> Option<Vec<u8, 258>> {
        let mut body = Vec::<u8, 255>::new();
        match self {
            Self::Status => body.push(0).ok()?,
            Self::Cancel => body.push(2).ok()?,
            Self::Excursion {
                target,
                duration_ms,
                allow_loss,
            } => {
                body.extend_from_slice(&[1, *target]).ok()?;
                body.extend_from_slice(&duration_ms.to_le_bytes()).ok()?;
                body.push(u8::from(*allow_loss)).ok()?;
            }
            Self::SennetText {
                destination,
                hops,
                want_ack,
                text,
            } => {
                body.push(3).ok()?;
                body.extend_from_slice(&destination.to_le_bytes()).ok()?;
                body.extend_from_slice(&[*hops, u8::from(*want_ack)]).ok()?;
                body.extend_from_slice(text.as_bytes()).ok()?;
            }
            Self::TucketText {
                to,
                timestamp,
                ttl_ms,
                attempts,
                flood_last,
                text,
            } => {
                body.extend_from_slice(&[4, *to]).ok()?;
                body.extend_from_slice(&timestamp.to_le_bytes()).ok()?;
                body.extend_from_slice(&ttl_ms.to_le_bytes()).ok()?;
                body.extend_from_slice(&[*attempts, u8::from(*flood_last)])
                    .ok()?;
                body.extend_from_slice(text.as_bytes()).ok()?;
            }
            Self::TucketAdvert { timestamp, data } => {
                body.push(5).ok()?;
                body.extend_from_slice(&timestamp.to_le_bytes()).ok()?;
                body.extend_from_slice(data).ok()?;
            }
        }
        Self::decode(&body)?;
        let mut frame = Vec::new();
        frame.push(MARKER).ok()?;
        frame
            .extend_from_slice(&(body.len() as u16).to_le_bytes())
            .ok()?;
        frame.extend_from_slice(&body).ok()?;
        Some(frame)
    }
}
pub struct Stream {
    body: [u8; MAX_BODY],
    phase: u8,
    len: u16,
    used: u16,
}
impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}
impl Stream {
    pub const fn new() -> Self {
        Self {
            body: [0; MAX_BODY],
            phase: 0,
            len: 0,
            used: 0,
        }
    }
    pub fn push(&mut self, byte: u8) -> Event {
        match self.phase {
            0 => {
                if byte == MARKER {
                    self.phase = 1;
                    Event::Pending
                } else if byte == 0 {
                    Event::Pending
                } else {
                    Event::Rejected
                }
            }
            1 => {
                self.len = u16::from(byte);
                self.phase = 2;
                Event::Pending
            }
            2 => {
                self.len |= u16::from(byte) << 8;
                self.used = 0;
                if self.len == 0 {
                    self.phase = 0;
                    Event::Rejected
                } else {
                    self.phase = 3;
                    Event::Pending
                }
            }
            _ => {
                if self.len as usize <= MAX_BODY {
                    self.body[self.used as usize] = byte;
                }
                self.used += 1;
                if self.used != self.len {
                    return Event::Pending;
                }
                self.phase = 0;
                if self.len as usize > MAX_BODY {
                    return Event::Rejected;
                }
                match Command::decode(&self.body[..self.len as usize]) {
                    Some(command) => Event::Complete(command),
                    None => Event::Rejected,
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_commands_and_payload_markers_remain_data() {
        let command = Command::SennetText {
            destination: u32::MAX,
            hops: 3,
            want_ack: true,
            text: String::try_from("hello\u{8}\0").unwrap(),
        };
        let frame = command.encode().unwrap();
        let mut stream = Stream::new();
        for &byte in &frame[..frame.len() - 1] {
            assert_eq!(stream.push(byte), Event::Pending);
        }
        assert_eq!(
            stream.push(*frame.last().unwrap()),
            Event::Complete(command)
        );
        for byte in Command::Cancel.encode().unwrap() {
            if let Event::Complete(c) = stream.push(byte) {
                assert_eq!(c, Command::Cancel);
            }
        }
    }
    #[test]
    fn overlong_body_is_drained_without_interpreting_embedded_commands() {
        let mut stream = Stream::new();
        for b in [MARKER, 0, 1] {
            assert_eq!(stream.push(b), Event::Pending);
        }
        for _ in 0..255 {
            assert_eq!(stream.push(MARKER), Event::Pending);
        }
        assert_eq!(stream.push(MARKER), Event::Rejected);
        let mut result = Event::Pending;
        for b in Command::Status.encode().unwrap() {
            result = stream.push(b);
        }
        assert_eq!(result, Event::Complete(Command::Status));
    }
    #[test]
    fn invalid_flags_lengths_text_and_retry_counts_are_refused() {
        assert!(Command::decode(&[0, 0]).is_none());
        assert!(Command::decode(&[3, 0, 0, 0, 0, 8, 0]).is_none());
        assert!(Command::decode(&[3, 0, 0, 0, 0, 1, 0, 255]).is_none());
        let invalid = Command::TucketText {
            to: 1,
            timestamp: 2,
            ttl_ms: 10,
            attempts: 5,
            flood_last: false,
            text: String::new(),
        };
        assert!(invalid.encode().is_none());
    }
}
