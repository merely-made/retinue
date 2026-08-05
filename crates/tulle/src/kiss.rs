//! KISS framing, host side: `Vec`-backed encoding and deframing.
//!
//! The escape rules themselves live in [`selvage::kiss`], because the board's RNode channel
//! obeys the same ones with no allocator. What is here is only the storage decision a host
//! gets to make differently: an encoder that returns a fresh `Vec`, and a deframer whose
//! bound is a runtime number rather than a const generic.

pub use selvage::kiss::{FEND, FESC, TFEND, TFESC};
use selvage::kiss::{Scan, Step, encode_into, encoded_len};

/// Encode one frame: leading + trailing FEND, contents escaped.
pub fn encode(frame: &[u8]) -> Vec<u8> {
    let mut out = vec![0_u8; encoded_len(frame)];
    let len = encode_into(frame, &mut out).expect("the buffer is sized from the frame");
    out.truncate(len);
    out
}

/// Streaming deframer with a bounded frame size.
///
/// Feed raw serial bytes in; complete frames come out. Frames that exceed `max_frame` are
/// discarded and the deframer resyncs at the next FEND, so a corrupt stream cannot balloon
/// memory. Invalid escape sequences discard the frame for the same reason.
pub struct Deframer {
    buf: Vec<u8>,
    max_frame: usize,
    scan: Scan,
    poisoned: bool,
}

impl Deframer {
    pub fn new(max_frame: usize) -> Self {
        Deframer {
            buf: Vec::new(),
            max_frame,
            scan: Scan::new(),
            poisoned: false,
        }
    }

    /// Consume raw bytes, appending any completed frames to `out`.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<Vec<u8>>) {
        for &byte in bytes {
            match self.scan.step(byte) {
                Step::Skip => {}
                Step::Byte(decoded) => {
                    if self.buf.len() >= self.max_frame {
                        self.poisoned = true;
                        self.buf.clear();
                    } else if !self.poisoned {
                        self.buf.push(decoded);
                    }
                }
                Step::Bad => {
                    self.poisoned = true;
                    self.buf.clear();
                }
                Step::End => {
                    if !self.poisoned && !self.buf.is_empty() {
                        out.push(std::mem::take(&mut self.buf));
                    } else {
                        self.buf.clear();
                    }
                    self.poisoned = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deframe_all(deframer: &mut Deframer, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        deframer.push(bytes, &mut out);
        out
    }

    #[test]
    fn roundtrip_plain() {
        let frame = b"\x01hello radio";
        let wire = encode(frame);
        let mut d = Deframer::new(512);
        let got = deframe_all(&mut d, &wire);
        assert_eq!(got, vec![frame.to_vec()]);
    }

    #[test]
    fn roundtrip_with_escapes() {
        let frame = vec![FEND, 0x42, FESC, FEND, FESC];
        let wire = encode(&frame);
        assert!(!wire[1..wire.len() - 1].contains(&FEND));
        let mut d = Deframer::new(512);
        assert_eq!(deframe_all(&mut d, &wire), vec![frame]);
    }

    #[test]
    fn split_across_pushes() {
        let frame = vec![9u8; 40];
        let wire = encode(&frame);
        let mut d = Deframer::new(512);
        let mut out = Vec::new();
        for chunk in wire.chunks(7) {
            d.push(chunk, &mut out);
        }
        assert_eq!(out, vec![frame]);
    }

    #[test]
    fn back_to_back_frames_share_fend() {
        // ... FEND a FEND b FEND: middle FEND ends one frame and opens the next
        let mut wire = encode(b"aa");
        wire.extend_from_slice(&encode(b"bb")[1..]); // drop duplicated FEND
        let mut d = Deframer::new(512);
        assert_eq!(
            deframe_all(&mut d, &wire),
            vec![b"aa".to_vec(), b"bb".to_vec()]
        );
    }

    #[test]
    fn oversize_frame_discarded_and_resyncs() {
        let big = vec![1u8; 600];
        let ok = b"fine".to_vec();
        let mut wire = encode(&big);
        wire.extend_from_slice(&encode(&ok));
        let mut d = Deframer::new(512);
        assert_eq!(deframe_all(&mut d, &wire), vec![ok]);
    }

    #[test]
    fn invalid_escape_discards_frame() {
        let wire = [FEND, 0x01, FESC, 0x99, 0x02, FEND, 0x33, FEND];
        let mut d = Deframer::new(512);
        assert_eq!(deframe_all(&mut d, &wire), vec![vec![0x33]]);
    }

    #[test]
    fn idle_fends_produce_nothing() {
        let mut d = Deframer::new(512);
        assert!(deframe_all(&mut d, &[FEND, FEND, FEND]).is_empty());
    }
}
