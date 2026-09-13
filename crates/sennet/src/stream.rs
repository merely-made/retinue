//! Bounded serial/TCP Stream API framing.

use alloc::vec::Vec;

pub const START1: u8 = 0x94;
pub const START2: u8 = 0xc3;
pub const MAX_PAYLOAD: usize = 512;
pub const DEFAULT_INPUT_CAPACITY: usize = 4 + MAX_PAYLOAD;
pub const DEFAULT_MAX_FRAMES_PER_PUSH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeframerConfig {
    pub max_payload: usize,
    pub max_frames_per_push: usize,
}

impl Default for DeframerConfig {
    fn default() -> Self {
        Self {
            max_payload: MAX_PAYLOAD,
            max_frames_per_push: DEFAULT_MAX_FRAMES_PER_PUSH,
        }
    }
}

/// Wrap a payload in the documented magic, length, payload frame.
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, StreamError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(StreamError::PayloadTooLong {
            actual: payload.len(),
            limit: MAX_PAYLOAD,
        });
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&[START1, START2]);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// A deframer retaining at most `4 + config.max_payload` bytes.
///
/// A completed-frame quota is explicit. On [`StreamError::OutputFull`], valid
/// input was not dropped: previously emitted frames are in `out`, the next
/// complete frame remains retained, and `consumed` identifies the input suffix
/// to retry after the caller drains `out`.
pub struct Deframer {
    config: DeframerConfig,
    buf: Vec<u8>,
}

impl Deframer {
    pub fn new() -> Self {
        Self::with_config(DeframerConfig::default()).expect("default stream configuration is valid")
    }

    pub fn with_config(config: DeframerConfig) -> Result<Self, StreamConfigError> {
        if config.max_payload > MAX_PAYLOAD {
            return Err(StreamConfigError::PayloadLimit(config.max_payload));
        }
        if config.max_frames_per_push == 0 {
            return Err(StreamConfigError::FrameLimit);
        }
        Ok(Self {
            buf: Vec::with_capacity(4 + config.max_payload),
            config,
        })
    }

    pub const fn config(&self) -> DeframerConfig {
        self.config
    }

    /// Appends at most `max_frames_per_push` frames, returning consumed input bytes.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<Vec<u8>>) -> Result<usize, StreamError> {
        let mut consumed = 0;
        let mut emitted = 0;
        loop {
            if self.frame_is_complete() && emitted == self.config.max_frames_per_push {
                return Err(StreamError::OutputFull {
                    limit: self.config.max_frames_per_push,
                    consumed,
                });
            }
            if let Some(frame) = self.ready_frame() {
                out.push(frame);
                emitted += 1;
                continue;
            }
            let Some(&byte) = bytes.get(consumed) else {
                return Ok(consumed);
            };
            self.buf.push(byte);
            consumed += 1;
            self.resynchronize();
        }
    }

    fn ready_frame(&mut self) -> Option<Vec<u8>> {
        if self.buf.len() < 4 || self.buf[0..2] != [START1, START2] {
            return None;
        }
        let len = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
        if len > self.config.max_payload {
            self.buf.drain(..2);
            return None;
        }
        if self.buf.len() < 4 + len {
            return None;
        }
        let frame = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Some(frame)
    }

    fn frame_is_complete(&self) -> bool {
        if self.buf.len() < 4 || self.buf[0..2] != [START1, START2] {
            return false;
        }
        let len = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
        len <= self.config.max_payload && self.buf.len() >= 4 + len
    }

    fn resynchronize(&mut self) {
        let Some(start) = find_magic(&self.buf) else {
            if self.buf.last() == Some(&START1) {
                self.buf.drain(..self.buf.len() - 1);
            } else {
                self.buf.clear();
            }
            return;
        };
        if start > 0 {
            self.buf.drain(..start);
        }
    }
}

impl Default for Deframer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamConfigError {
    PayloadLimit(usize),
    FrameLimit,
}

impl core::fmt::Display for StreamConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PayloadLimit(limit) => {
                write!(f, "stream payload limit exceeds {MAX_PAYLOAD}: {limit}")
            }
            Self::FrameLimit => write!(f, "stream completed-frame limit must be non-zero"),
        }
    }
}
impl core::error::Error for StreamConfigError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamError {
    PayloadTooLong { actual: usize, limit: usize },
    OutputFull { limit: usize, consumed: usize },
}
impl core::fmt::Display for StreamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PayloadTooLong { actual, limit } => {
                write!(f, "stream payload exceeds {limit} bytes: {actual}")
            }
            Self::OutputFull { limit, consumed } => write!(
                f,
                "stream output limit {limit} reached after {consumed} input bytes"
            ),
        }
    }
}
impl core::error::Error for StreamError {}

fn find_magic(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == [START1, START2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn all(d: &mut Deframer, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        assert_eq!(d.push(bytes, &mut out), Ok(bytes.len()));
        out
    }
    #[test]
    fn round_trips() {
        let mut d = Deframer::new();
        assert_eq!(all(&mut d, &encode(b"hi").unwrap()), vec![b"hi".to_vec()]);
    }
    #[test]
    fn skips_leading_noise_and_preserves_a_split_magic() {
        let mut d = Deframer::new();
        assert!(all(&mut d, &[0, START1]).is_empty());
        let mut tail = vec![START2, 0, 2, b'h', b'i'];
        tail.extend_from_slice(&encode(b"there").unwrap());
        assert_eq!(all(&mut d, &tail), vec![b"hi".to_vec(), b"there".to_vec()]);
    }
    #[test]
    fn chunks() {
        let wire = encode(&vec![7; 300]).unwrap();
        let mut d = Deframer::new();
        let mut out = Vec::new();
        for chunk in wire.chunks(17) {
            d.push(chunk, &mut out).unwrap();
        }
        assert_eq!(out, vec![vec![7; 300]]);
    }
    #[test]
    fn corrupt_length_resyncs() {
        let mut wire = vec![START1, START2, 0xff, 0xff];
        wire.extend_from_slice(&encode(b"real").unwrap());
        let mut d = Deframer::new();
        assert_eq!(all(&mut d, &wire), vec![b"real".to_vec()]);
    }
    #[test]
    fn quota_preserves_coalesced_frames() {
        let mut wire = Vec::new();
        for p in [b"one".as_slice(), b"two", b"three"] {
            wire.extend_from_slice(&encode(p).unwrap());
        }
        let mut d = Deframer::with_config(DeframerConfig {
            max_payload: MAX_PAYLOAD,
            max_frames_per_push: 2,
        })
        .unwrap();
        let mut out = Vec::new();
        let consumed = match d.push(&wire, &mut out) {
            Err(StreamError::OutputFull { consumed, .. }) => consumed,
            value => panic!("expected OutputFull, got {value:?}"),
        };
        assert_eq!(out, vec![b"one".to_vec(), b"two".to_vec()]);
        out.clear();
        assert_eq!(
            d.push(&wire[consumed..], &mut out),
            Ok(wire.len() - consumed)
        );
        assert_eq!(out, vec![b"three".to_vec()]);
    }
    #[test]
    fn oversized_encode_is_refused() {
        assert!(matches!(
            encode(&vec![0; MAX_PAYLOAD + 1]),
            Err(StreamError::PayloadTooLong { .. })
        ));
    }
}
