//! KISS framing: the public amateur-radio spec the RNode host protocol builds on.
//!
//! `FEND` delimits frames; a literal `FEND` inside a frame is escaped as `FESC TFEND` and a
//! literal `FESC` as `FESC TFESC`. Nothing about frame *contents* lives here — the command
//! byte and opcode set are the RNode layer's business, pinned by hardware capture.
//!
//! # Why it sits in selvage
//!
//! Both ends of that protocol are in this workspace now: `tulle` drives an RNode from a
//! computer, and the board's RNode channel *is* one. The escape rules are the same rules on
//! both sides, and a second copy of them is a second place for a resync bug to hide. So the
//! rules live here, where host and firmware already share code, and each side keeps the
//! buffer that suits it: `tulle` grows a `Vec`, the board takes a fixed array.

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// Bytes [`encode_into`] writes for `frame`, delimiters included.
pub fn encoded_len(frame: &[u8]) -> usize {
    let escapes = frame.iter().filter(|&&b| b == FEND || b == FESC).count();
    frame.len() + escapes + 2
}

/// Encode one frame: leading and trailing `FEND`, contents escaped.
///
/// Returns the bytes written, or `None` if `out` is too small. Refusing rather than
/// truncating: half a KISS frame is not a shorter frame, it is a frame the far end will
/// join to the next one.
pub fn encode_into(frame: &[u8], out: &mut [u8]) -> Option<usize> {
    encode_pair_into(frame, &[], out)
}

/// Encode one frame given in two pieces.
///
/// Which is what every RNode frame is: a command byte, then a payload the caller already
/// holds somewhere else. Joining them first would mean a scratch buffer as large as the
/// largest frame, existing only to be copied out of.
pub fn encode_pair_into(head: &[u8], tail: &[u8], out: &mut [u8]) -> Option<usize> {
    if out.len() < encoded_len(head) + encoded_len(tail) - 2 {
        return None;
    }
    let mut at = 0;
    let mut put = |byte: u8| {
        out[at] = byte;
        at += 1;
    };
    put(FEND);
    for &byte in head.iter().chain(tail) {
        match byte {
            FEND => {
                put(FESC);
                put(TFEND);
            }
            FESC => {
                put(FESC);
                put(TFESC);
            }
            _ => put(byte),
        }
    }
    put(FEND);
    Some(at)
}

/// What one byte means to a deframer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    /// Consumed, nothing to store: the first half of an escape sequence.
    Skip,
    /// Store this byte in the frame under construction.
    Byte(u8),
    /// A `FEND`. Whatever has been stored is a complete frame, if anything has.
    End,
    /// An invalid escape sequence. The frame is unrecoverable; discard it and resync at the
    /// next [`Step::End`]. Bounding memory is the reason this is not simply ignored.
    Bad,
}

/// The framing and escape state machine, one byte at a time.
///
/// Deliberately does not own a buffer, so a caller with 256 KB of RAM and a caller with an
/// allocator can share the rules without sharing the storage decision.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scan {
    /// Whether a `FEND` has been seen at all. Until one has, there is no frame to be inside.
    in_frame: bool,
    in_escape: bool,
}

impl Scan {
    pub const fn new() -> Self {
        Self {
            in_frame: false,
            in_escape: false,
        }
    }

    /// Whether a frame is mid-escape, and so cannot end cleanly here.
    pub const fn is_escaping(&self) -> bool {
        self.in_escape
    }

    pub fn step(&mut self, byte: u8) -> Step {
        if byte == FEND {
            // A FEND during an escape ends the frame anyway: the escape was truncated, so
            // the delimiter is the more trustworthy of the two.
            self.in_escape = false;
            self.in_frame = true;
            return Step::End;
        }
        // A frame starts at a FEND, so bytes before the first one are not frame contents.
        //
        // Not pedantry about the spec: on a board this stream is shared with text probes,
        // and a lenient deframer accumulating stray text would leave itself permanently
        // mid-frame. The probe that switches channels would then stop being recognised, and
        // the only way out of a channel would be a reflash.
        if !self.in_frame {
            return Step::Skip;
        }
        if self.in_escape {
            self.in_escape = false;
            return match byte {
                TFEND => Step::Byte(FEND),
                TFESC => Step::Byte(FESC),
                _ => Step::Bad,
            };
        }
        if byte == FESC {
            self.in_escape = true;
            return Step::Skip;
        }
        Step::Byte(byte)
    }
}

/// A deframer over a fixed buffer, for callers with no allocator.
///
/// `N` is the largest frame that survives. Anything longer is discarded whole and the stream
/// resyncs at the next `FEND`, so a peer cannot make this grow.
pub struct Deframer<const N: usize> {
    buffer: [u8; N],
    len: usize,
    scan: Scan,
    /// The frame under construction is unrecoverable: too long, or badly escaped.
    poisoned: bool,
    /// [`Deframer::frame`] holds a complete frame, until the next byte is pushed.
    ready: bool,
}

impl<const N: usize> Default for Deframer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Deframer<N> {
    pub const fn new() -> Self {
        Self {
            buffer: [0; N],
            len: 0,
            scan: Scan::new(),
            poisoned: false,
            ready: false,
        }
    }

    /// Feed one byte. `true` when [`Deframer::frame`] now holds a complete frame.
    ///
    /// The frame stays readable only until the next push, which is what lets one buffer serve
    /// both the frame under construction and the finished one.
    pub fn push(&mut self, byte: u8) -> bool {
        if self.ready {
            self.ready = false;
            self.len = 0;
        }
        match self.scan.step(byte) {
            Step::Skip => {}
            Step::Byte(decoded) => {
                if self.len == N {
                    self.poisoned = true;
                } else if !self.poisoned {
                    self.buffer[self.len] = decoded;
                    self.len += 1;
                }
            }
            Step::Bad => self.poisoned = true,
            Step::End => {
                let complete = !self.poisoned && self.len > 0;
                self.poisoned = false;
                if complete {
                    self.ready = true;
                    return true;
                }
                self.len = 0;
            }
        }
        false
    }

    /// The frame the last [`Deframer::push`] completed.
    pub fn frame(&self) -> &[u8] {
        if self.ready {
            &self.buffer[..self.len]
        } else {
            &[]
        }
    }

    /// Whether nothing is half-read.
    ///
    /// What a firmware sharing one byte stream between KISS frames and text probes asks
    /// before treating bytes as text: a `status` line inside a transmit payload must be
    /// carried, not obeyed.
    pub fn is_idle(&self) -> bool {
        (self.ready || self.len == 0) && !self.poisoned && !self.scan.is_escaping()
    }

    /// Forget anything half-read. A new host session starts at a frame boundary whatever the
    /// last one left behind.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every complete frame in `bytes`, as (count, contents) so a test can assert both.
    ///
    /// Fixed arrays rather than a collection: selvage has no dependencies, and keeping it
    /// that way is worth a little test scaffolding.
    fn frames<const N: usize>(deframer: &mut Deframer<N>, bytes: &[u8]) -> (usize, [[u8; 48]; 4]) {
        let mut out = [[0_u8; 48]; 4];
        let mut count = 0;
        for &byte in bytes {
            if deframer.push(byte) {
                let frame = deframer.frame();
                out[count][..frame.len()].copy_from_slice(frame);
                count += 1;
            }
        }
        (count, out)
    }

    fn encode(frame: &[u8], out: &mut [u8; 64]) -> usize {
        encode_into(frame, out).unwrap()
    }

    #[test]
    fn round_trips_plain_bytes() {
        let mut wire = [0_u8; 64];
        let len = encode(b"\x01hello radio", &mut wire);
        let mut deframer = Deframer::<64>::new();
        let (count, got) = frames(&mut deframer, &wire[..len]);
        assert_eq!(count, 1);
        assert_eq!(&got[0][..12], b"\x01hello radio");
    }

    #[test]
    fn round_trips_bytes_needing_escapes() {
        let frame = [FEND, 0x42, FESC, FEND, FESC];
        let mut wire = [0_u8; 64];
        let len = encode(&frame, &mut wire);
        assert!(
            !wire[1..len - 1].contains(&FEND),
            "no bare delimiter inside"
        );
        let mut deframer = Deframer::<64>::new();
        let (count, got) = frames(&mut deframer, &wire[..len]);
        assert_eq!(count, 1);
        assert_eq!(&got[0][..5], &frame);
    }

    #[test]
    fn back_to_back_frames_share_one_delimiter() {
        let mut first = [0_u8; 64];
        let first_len = encode(b"aa", &mut first);
        let mut second = [0_u8; 64];
        let second_len = encode(b"bb", &mut second);
        let mut wire = [0_u8; 64];
        wire[..first_len].copy_from_slice(&first[..first_len]);
        wire[first_len..first_len + second_len - 1].copy_from_slice(&second[1..second_len]);

        let mut deframer = Deframer::<64>::new();
        let (count, got) = frames(&mut deframer, &wire[..first_len + second_len - 1]);
        assert_eq!(count, 2);
        assert_eq!(&got[0][..2], b"aa");
        assert_eq!(&got[1][..2], b"bb");
    }

    /// A frame longer than the buffer is dropped whole and the next one still arrives. This
    /// is the property that keeps a peer from deciding how much memory this costs.
    #[test]
    fn an_oversize_frame_is_discarded_and_the_stream_resyncs() {
        let mut wire = [0_u8; 64];
        let big_len = encode(&[1_u8; 40], &mut wire);
        let mut good = [0_u8; 64];
        let good_len = encode(b"fine", &mut good);
        wire[big_len..big_len + good_len].copy_from_slice(&good[..good_len]);

        let mut deframer = Deframer::<16>::new();
        let (count, got) = frames(&mut deframer, &wire[..big_len + good_len]);
        assert_eq!(count, 1);
        assert_eq!(&got[0][..4], b"fine");
    }

    #[test]
    fn an_invalid_escape_discards_its_frame_only() {
        let wire = [FEND, 0x01, FESC, 0x99, 0x02, FEND, 0x33, FEND];
        let mut deframer = Deframer::<16>::new();
        let (count, got) = frames(&mut deframer, &wire);
        assert_eq!(count, 1);
        assert_eq!(got[0][0], 0x33);
    }

    #[test]
    fn idle_delimiters_produce_nothing() {
        let mut deframer = Deframer::<16>::new();
        assert_eq!(frames(&mut deframer, &[FEND, FEND, FEND]).0, 0);
    }

    /// Stray bytes before the first delimiter are not frame contents, and the frame after
    /// them still arrives whole.
    ///
    /// The board depends on this: it shares one byte stream between KISS frames and text
    /// probes, and a deframer that swallowed a mistyped line would report itself
    /// permanently mid-frame. The probe that switches channels would stop being recognised
    /// and the only way out of the channel would be a reflash.
    #[test]
    fn bytes_before_the_first_delimiter_are_ignored() {
        let mut wire = [0_u8; 64];
        let text = b"channel modem\r\n";
        wire[..text.len()].copy_from_slice(text);
        let mut frame = [0_u8; 64];
        let frame_len = encode(b"\x08\x73", &mut frame);
        wire[text.len()..text.len() + frame_len].copy_from_slice(&frame[..frame_len]);

        let mut deframer = Deframer::<32>::new();
        for &byte in &wire[..text.len()] {
            assert!(!deframer.push(byte));
            assert!(deframer.is_idle(), "stray text never opens a frame");
        }
        let (count, got) = frames(&mut deframer, &wire[text.len()..text.len() + frame_len]);
        assert_eq!(count, 1);
        assert_eq!(&got[0][..2], b"\x08\x73");
    }

    /// The boundary question a firmware asks before reading bytes as text.
    #[test]
    fn a_half_read_frame_is_not_a_boundary() {
        let mut deframer = Deframer::<16>::new();
        assert!(deframer.is_idle(), "nothing read yet");
        deframer.push(FEND);
        assert!(deframer.is_idle(), "an empty frame is still a boundary");
        deframer.push(0x01);
        assert!(!deframer.is_idle(), "a frame is half-read");
        deframer.push(FESC);
        assert!(!deframer.is_idle(), "and mid-escape is worse");
        deframer.push(TFEND);
        assert!(deframer.push(FEND), "the frame completes");
        assert!(deframer.is_idle(), "and the stream is back at a boundary");
    }

    /// A command byte plus a payload encodes exactly as the joined frame would, escapes
    /// included, which is what lets the board skip the joining buffer.
    #[test]
    fn a_frame_in_two_pieces_encodes_as_one() {
        let mut joined = [0_u8; 64];
        let joined_len = encode(&[0x00, FEND, 0x42, FESC], &mut joined);
        let mut split = [0_u8; 64];
        let split_len = encode_pair_into(&[0x00], &[FEND, 0x42, FESC], &mut split).unwrap();
        assert_eq!(&split[..split_len], &joined[..joined_len]);
    }

    #[test]
    fn encoding_refuses_a_buffer_it_would_overrun() {
        let mut out = [0_u8; 4];
        assert_eq!(encode_into(b"abc", &mut out), None);
        assert_eq!(encode_into(b"ab", &mut out), Some(4));
    }
}
