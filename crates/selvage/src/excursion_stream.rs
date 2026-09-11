//! A bounded side channel for one complete host-requested PHY excursion.
//!
//! The caller supplies whether the ordinary command parser is at a frame
//! boundary for each byte. Only `CMD_EXCURSION` at that boundary starts a
//! candidate. Once started, every following byte is literal: a KISS delimiter,
//! wake byte, ordinary command marker, or zero byte cannot escape the fixed
//! command body. Feed coalesced reads one byte at a time; each call returns the
//! byte for the ordinary parser, a held candidate byte, or one complete command.
//!
//! If the caller aborts its ordinary parser while a candidate is incomplete, it
//! must call [`ExcursionStream::abort`]. That discards the candidate rather than
//! replaying its prefix into a newly reset ordinary parser, since replay would
//! reinterpret already accepted framing bytes.

use crate::{CMD_EXCURSION, EXCURSION_COMMAND_LEN};

/// One byte's routing result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcursionByte {
    /// Leave this byte with the ordinary command parser.
    Ordinary(u8),
    /// Hold this byte while collecting an excursion command.
    Pending,
    /// A complete, fixed-size excursion command for the board owner.
    Complete([u8; EXCURSION_COMMAND_LEN]),
}

/// Fixed-size excursion command collector, independent of transport framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcursionStream {
    candidate: [u8; EXCURSION_COMMAND_LEN],
    len: usize,
}

impl Default for ExcursionStream {
    fn default() -> Self {
        Self::new()
    }
}

impl ExcursionStream {
    pub const fn new() -> Self {
        Self {
            candidate: [0; EXCURSION_COMMAND_LEN],
            len: 0,
        }
    }

    /// Whether an excursion candidate owns subsequent literal bytes.
    pub const fn pending(&self) -> bool {
        self.len != 0
    }

    /// Discard an incomplete candidate when the caller aborts ordinary framing.
    pub fn abort(&mut self) {
        self.len = 0;
    }

    /// Route one host byte.
    ///
    /// `ordinary_boundary` is consulted only while idle. A candidate captures
    /// all remaining bytes literally until its fixed body is complete.
    pub fn push(&mut self, ordinary_boundary: bool, byte: u8) -> ExcursionByte {
        if self.len == 0 {
            if !ordinary_boundary || byte != CMD_EXCURSION {
                return ExcursionByte::Ordinary(byte);
            }
            self.candidate[0] = byte;
            self.len = 1;
            return ExcursionByte::Pending;
        }

        self.candidate[self.len] = byte;
        self.len += 1;
        if self.len != EXCURSION_COMMAND_LEN {
            return ExcursionByte::Pending;
        }
        self.len = 0;
        ExcursionByte::Complete(self.candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> [u8; EXCURSION_COMMAND_LEN] {
        let mut command = [0_u8; EXCURSION_COMMAND_LEN];
        for (index, byte) in command.iter_mut().enumerate() {
            *byte = index as u8;
        }
        command[0] = CMD_EXCURSION;
        command
    }

    #[test]
    fn reassembles_every_fragmentation_and_leaves_coalesced_trailing_bytes() {
        let command = command();
        for split in 1..EXCURSION_COMMAND_LEN {
            let mut stream = ExcursionStream::new();
            for (index, byte) in command[..split].iter().enumerate() {
                assert_eq!(
                    stream.push(index == 0, *byte),
                    ExcursionByte::Pending,
                    "split {split}, byte {index}"
                );
            }
            for (index, byte) in command[split..].iter().enumerate() {
                let result = stream.push(false, *byte);
                if split + index + 1 == EXCURSION_COMMAND_LEN {
                    assert_eq!(result, ExcursionByte::Complete(command));
                } else {
                    assert_eq!(result, ExcursionByte::Pending);
                }
            }
            assert_eq!(stream.push(true, 0xA5), ExcursionByte::Ordinary(0xA5));
        }
    }

    #[test]
    fn candidate_treats_framing_and_marker_bytes_as_literal() {
        let mut command = command();
        command[3] = 0;
        command[4] = crate::kiss::FEND;
        command[5] = crate::WAKE_BYTE;
        command[6] = CMD_EXCURSION;
        let mut stream = ExcursionStream::new();
        for (index, byte) in command.iter().enumerate() {
            let result = stream.push(index == 0, *byte);
            if index + 1 == EXCURSION_COMMAND_LEN {
                assert_eq!(result, ExcursionByte::Complete(command));
            } else {
                assert_eq!(result, ExcursionByte::Pending);
            }
        }
    }

    #[test]
    fn does_not_capture_marker_inside_an_ordinary_tx_frame() {
        let mut stream = ExcursionStream::new();
        assert_eq!(
            stream.push(false, CMD_EXCURSION),
            ExcursionByte::Ordinary(CMD_EXCURSION)
        );
        assert!(!stream.pending());
    }

    #[test]
    fn abort_discards_an_incomplete_candidate() {
        let mut stream = ExcursionStream::new();
        assert_eq!(stream.push(true, CMD_EXCURSION), ExcursionByte::Pending);
        assert!(stream.pending());
        stream.abort();
        assert_eq!(stream.push(true, 0xA6), ExcursionByte::Ordinary(0xA6));
    }
}
