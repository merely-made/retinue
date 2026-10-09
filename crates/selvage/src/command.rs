//! Reassembly of direct-PHY commands from a fragmented host byte stream.

use crate::{
    CMD_CONFIG, CMD_EXCURSION, CMD_OBSERVATION, CMD_TX, CMD_UI_SNAPSHOT, CONFIG_COMMAND_LEN,
    EXCURSION_COMMAND_LEN, MAX_COMMAND_LEN, MAX_RADIO_FRAME_LEN, MAX_UI_SNAPSHOT_LEN, WAKE_BYTE,
};

/// A complete command recovered from an arbitrarily fragmented host byte
/// stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    Transmit,
    Configure,
    UiSnapshot,
    Observation,
    Excursion,
}

/// Result of feeding one byte to [`CommandStream`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandEvent {
    Pending,
    Complete { kind: CommandKind, len: usize },
    TooLong { kind: CommandKind, declared: usize },
    Unknown { marker: u8 },
}

/// Bounded direct-PHY command reassembler.
///
/// Transmit commands remain length-prefixed. UI snapshots use a zero-free,
/// zero-delimited body so an interrupted snapshot can be rejected and the
/// following wake-prefixed command still begins at a clean boundary.
pub struct CommandStream {
    buffer: [u8; MAX_COMMAND_LEN],
    len: usize,
    expected: usize,
    discarding: usize,
    discard_until_boundary: bool,
}

impl CommandStream {
    pub const fn new() -> Self {
        Self {
            buffer: [0; MAX_COMMAND_LEN],
            len: 0,
            expected: 0,
            discarding: 0,
            discard_until_boundary: false,
        }
    }

    pub fn is_boundary(&self) -> bool {
        self.len == 0 && self.discarding == 0 && !self.discard_until_boundary
    }

    pub fn push(&mut self, byte: u8, command: &mut [u8; MAX_COMMAND_LEN]) -> CommandEvent {
        if self.discarding > 0 {
            self.discarding -= 1;
            return CommandEvent::Pending;
        }
        if self.discard_until_boundary {
            if byte == WAKE_BYTE {
                self.discard_until_boundary = false;
            }
            return CommandEvent::Pending;
        }

        if self.len == 0 {
            if byte == WAKE_BYTE {
                return CommandEvent::Pending;
            }
            let kind = match byte {
                CMD_TX => CommandKind::Transmit,
                CMD_CONFIG => CommandKind::Configure,
                CMD_UI_SNAPSHOT => CommandKind::UiSnapshot,
                CMD_OBSERVATION => CommandKind::Observation,
                CMD_EXCURSION => CommandKind::Excursion,
                marker => return CommandEvent::Unknown { marker },
            };
            self.buffer[0] = byte;
            self.len = 1;
            self.expected = match kind {
                CommandKind::Configure => CONFIG_COMMAND_LEN,
                CommandKind::Excursion => EXCURSION_COMMAND_LEN,
                CommandKind::Transmit | CommandKind::UiSnapshot | CommandKind::Observation => 0,
            };
            return CommandEvent::Pending;
        }

        let kind = match self.buffer[0] {
            CMD_TX => CommandKind::Transmit,
            CMD_CONFIG => CommandKind::Configure,
            CMD_UI_SNAPSHOT => CommandKind::UiSnapshot,
            CMD_OBSERVATION => CommandKind::Observation,
            CMD_EXCURSION => CommandKind::Excursion,
            _ => unreachable!("only known command markers enter the buffer"),
        };

        if matches!(kind, CommandKind::UiSnapshot | CommandKind::Observation) && byte == WAKE_BYTE {
            let len = self.len;
            command[..len].copy_from_slice(&self.buffer[..len]);
            self.len = 0;
            self.expected = 0;
            return CommandEvent::Complete { kind, len };
        }

        if self.len == self.buffer.len() {
            self.len = 0;
            self.expected = 0;
            self.discard_until_boundary = true;
            return CommandEvent::TooLong {
                kind,
                declared: MAX_UI_SNAPSHOT_LEN + 1,
            };
        }

        self.buffer[self.len] = byte;
        self.len += 1;

        if kind == CommandKind::Transmit && self.expected == 0 && self.len == 3 {
            let declared = usize::from(u16::from_le_bytes([self.buffer[1], self.buffer[2]]));
            if declared > MAX_RADIO_FRAME_LEN {
                self.len = 0;
                self.expected = 0;
                self.discarding = declared;
                return CommandEvent::TooLong { kind, declared };
            }
            self.expected = 3 + declared;
        }

        if self.len != self.expected {
            return CommandEvent::Pending;
        }

        let len = self.len;
        command[..len].copy_from_slice(&self.buffer[..len]);
        self.len = 0;
        self.expected = 0;
        CommandEvent::Complete { kind, len }
    }
}

impl Default for CommandStream {
    fn default() -> Self {
        Self::new()
    }
}
