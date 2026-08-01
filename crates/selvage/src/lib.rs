#![no_std]

/// Meshtastic's documented LoRa synchronization byte.
pub const MESHTASTIC_SYNC_WORD: u8 = 0x2b;
/// MeshCore's private-network LoRa synchronization byte.
pub const MESHCORE_SYNC_WORD: u8 = 0x12;

/// Direct-PHY host-to-firmware command markers.
pub const CMD_TX: u8 = 0x01;
pub const CMD_CONFIG: u8 = 0x02;
/// Publish one versioned, explicitly lossy host snapshot to the local UI.
///
/// The payload is owned by `radio-face`; this transport crate treats it as
/// opaque bytes.
pub const CMD_UI_SNAPSHOT: u8 = 0x03;

/// Direct-PHY firmware-to-host event markers.
pub const EVENT_RX: u8 = 0x81;
pub const EVENT_TX: u8 = 0x82;
pub const EVENT_CONFIG: u8 = 0x83;
/// Firmware-to-host SX126x diagnostic event marker.
pub const EVENT_DIAGNOSTIC: u8 = 0x84;
/// Result of a [`CMD_UI_SNAPSHOT`] command.
pub const EVENT_UI_SNAPSHOT: u8 = 0x85;

/// Bytes in a complete [`CMD_CONFIG`] command.
pub const CONFIG_COMMAND_LEN: usize = 16;
/// Largest radio payload carried by [`CMD_TX`].
pub const MAX_RADIO_FRAME_LEN: usize = 255;
/// Largest opaque `radio-face` snapshot accepted by board firmware.
pub const MAX_UI_SNAPSHOT_LEN: usize = 160;
/// Marker plus the largest zero-free hexadecimal snapshot body.
pub const MAX_UI_SNAPSHOT_COMMAND_BODY_LEN: usize = 1 + 2 * MAX_UI_SNAPSHOT_LEN;
/// Largest complete UI-snapshot command, including its zero delimiter.
pub const MAX_UI_SNAPSHOT_COMMAND_LEN: usize = MAX_UI_SNAPSHOT_COMMAND_BODY_LEN + 1;
/// Largest command body retained by the stream parser.
pub const MAX_COMMAND_LEN: usize = MAX_UI_SNAPSHOT_COMMAND_BODY_LEN;

/// Results of a [`CMD_TX`] command, carried by [`EVENT_TX`].
///
/// Bare numbers in both firmware images until N2 named them, and reachable by a host,
/// so they belong beside the other wire results rather than in a board.
/// The frame reached the air.
pub const TX_ACCEPTED: u8 = 0;
/// The radio refused the frame, either preparing to transmit or transmitting.
pub const TX_RADIO_FAULT: u8 = 1;
/// The command marker is not one this firmware knows.
pub const TX_UNKNOWN_COMMAND: u8 = 3;
/// The declared frame is longer than [`MAX_RADIO_FRAME_LEN`].
pub const TX_TOO_LONG: u8 = 4;
/// Transmission was still unfinished when the firmware's deadline passed. The radio is
/// left in an unknown state, so a board that can read chip diagnostics should emit them.
pub const TX_TIMEOUT: u8 = 5;

/// Results of a [`CMD_CONFIG`] command, carried by [`EVENT_CONFIG`].
///
/// These were bare numbers written twice in the firmware images before N2 named them.
/// They sit here beside the snapshot results because both are wire values a host reads.
/// The profile was applied and the radio is on the new channel.
pub const CONFIG_ACCEPTED: u8 = 0;
/// The command did not decode: wrong length, or a field the codec rejected.
pub const CONFIG_MALFORMED: u8 = 1;
/// The profile decoded but names a setting this radio has no value for, or the driver
/// refused the resulting modulation or packet parameters. The old profile still stands.
pub const CONFIG_UNSUPPORTED: u8 = 2;
/// Parameters were accepted but the radio would not take the sync word, so the channel
/// is left in whatever state the driver reached. The host should reconfigure.
pub const CONFIG_RADIO_FAULT: u8 = 3;

pub const UI_SNAPSHOT_ACCEPTED: u8 = 0;
pub const UI_SNAPSHOT_MALFORMED: u8 = 1;
pub const UI_SNAPSHOT_UNSUPPORTED_VERSION: u8 = 2;
pub const UI_SNAPSHOT_TOO_LONG: u8 = 3;

/// The byte a host repeats to wake firmware whose host link sleeps.
///
/// A UART wake consumes the character that triggered it, and may lose the ones immediately
/// behind it, so a command sent cold can arrive truncated. The host instead sends a run of
/// this byte, waits for the link to settle, and only then sends the command.
///
/// It is deliberately not a valid command marker ([`CMD_TX`], [`CMD_CONFIG`]), so firmware can
/// discard it without ambiguity — but only at a frame boundary, since the same value is
/// perfectly legal *inside* a frame's length field or payload.
pub const WAKE_BYTE: u8 = 0x00;

const _: () =
    assert!(WAKE_BYTE != CMD_TX && WAKE_BYTE != CMD_CONFIG && WAKE_BYTE != CMD_UI_SNAPSHOT);

/// A complete command recovered from an arbitrarily fragmented host byte
/// stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    Transmit,
    Configure,
    UiSnapshot,
}

/// Result of feeding one byte to [`CommandStream`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandEvent {
    Pending,
    Complete { kind: CommandKind, len: usize },
    TooLong { kind: CommandKind, declared: usize },
    Unknown { marker: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiSnapshotWireError {
    TooLong,
    InvalidMarker,
    OddLength,
    InvalidHex,
}

/// Encode an opaque snapshot as `03 <lowercase-hex> 00`.
///
/// The zero-free body gives the stream parser an unambiguous recovery boundary
/// if an outer command is truncated. The next command's wake byte terminates
/// the damaged snapshot instead of becoming snapshot data.
pub fn encode_ui_snapshot_command(
    snapshot: &[u8],
    output: &mut [u8; MAX_UI_SNAPSHOT_COMMAND_LEN],
) -> Result<usize, UiSnapshotWireError> {
    if snapshot.len() > MAX_UI_SNAPSHOT_LEN {
        return Err(UiSnapshotWireError::TooLong);
    }
    output[0] = CMD_UI_SNAPSHOT;
    for (index, byte) in snapshot.iter().copied().enumerate() {
        output[1 + 2 * index] = hex(byte >> 4);
        output[2 + 2 * index] = hex(byte & 0x0f);
    }
    let len = 1 + 2 * snapshot.len();
    output[len] = WAKE_BYTE;
    Ok(len + 1)
}

/// Decode a complete UI-snapshot command body after its zero delimiter was
/// removed by [`CommandStream`].
pub fn decode_ui_snapshot_command(
    command: &[u8],
    output: &mut [u8; MAX_UI_SNAPSHOT_LEN],
) -> Result<usize, UiSnapshotWireError> {
    if command.first().copied() != Some(CMD_UI_SNAPSHOT) {
        return Err(UiSnapshotWireError::InvalidMarker);
    }
    let encoded = &command[1..];
    if encoded.len() > 2 * MAX_UI_SNAPSHOT_LEN {
        return Err(UiSnapshotWireError::TooLong);
    }
    if !encoded.len().is_multiple_of(2) {
        return Err(UiSnapshotWireError::OddLength);
    }
    for (index, pair) in encoded.chunks_exact(2).enumerate() {
        output[index] = unhex(pair[0])?.checked_shl(4).unwrap_or(0) | unhex(pair[1])?;
    }
    Ok(encoded.len() / 2)
}

const fn hex(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'a' + nibble - 10,
    }
}

const fn unhex(byte: u8) -> Result<u8, UiSnapshotWireError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(UiSnapshotWireError::InvalidHex),
    }
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
                marker => return CommandEvent::Unknown { marker },
            };
            self.buffer[0] = byte;
            self.len = 1;
            self.expected = match kind {
                CommandKind::Configure => CONFIG_COMMAND_LEN,
                CommandKind::Transmit | CommandKind::UiSnapshot => 0,
            };
            return CommandEvent::Pending;
        }

        let kind = match self.buffer[0] {
            CMD_TX => CommandKind::Transmit,
            CMD_CONFIG => CommandKind::Configure,
            CMD_UI_SNAPSHOT => CommandKind::UiSnapshot,
            _ => unreachable!("only known command markers enter the buffer"),
        };

        if kind == CommandKind::UiSnapshot && byte == WAKE_BYTE {
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

/// Radio parameters that are independent of a particular HAL or driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyProfile {
    pub frequency_hz: u32,
    pub bandwidth_hz: u32,
    pub spreading_factor: u8,
    pub coding_rate_denominator: u8,
    pub preamble_symbols: u16,
    pub sync_word: u8,
    pub explicit_header: bool,
    pub crc: bool,
    pub invert_iq: bool,
    pub tx_power_dbm: i8,
}

impl PhyProfile {
    /// Meshtastic LongFast modulation with a caller-selected regional frequency.
    pub const fn meshtastic_long_fast(frequency_hz: u32) -> Self {
        Self {
            frequency_hz,
            bandwidth_hz: 250_000,
            spreading_factor: 11,
            coding_rate_denominator: 5,
            preamble_symbols: 16,
            sync_word: MESHTASTIC_SYNC_WORD,
            explicit_header: true,
            crc: true,
            invert_iq: false,
            tx_power_dbm: 17,
        }
    }

    /// MeshCore modulation with caller-selected companion radio parameters.
    ///
    /// MeshCore lengthens the preamble to 32 symbols at SF5 through SF8 and
    /// otherwise uses 16. Frequency, bandwidth, spreading factor, and coding
    /// rate remain network settings rather than board defaults.
    pub const fn meshcore(
        frequency_hz: u32,
        bandwidth_hz: u32,
        spreading_factor: u8,
        coding_rate_denominator: u8,
    ) -> Self {
        Self {
            frequency_hz,
            bandwidth_hz,
            spreading_factor,
            coding_rate_denominator,
            preamble_symbols: if spreading_factor <= 8 { 32 } else { 16 },
            sync_word: MESHCORE_SYNC_WORD,
            explicit_header: true,
            crc: true,
            invert_iq: false,
            tx_power_dbm: 17,
        }
    }

    /// Validate the protocol-independent envelope accepted by Tulle firmware.
    pub const fn validate(self) -> Result<Self, ProfileError> {
        if self.frequency_hz == 0 {
            return Err(ProfileError::Frequency);
        }
        if self.bandwidth_hz == 0 {
            return Err(ProfileError::Bandwidth);
        }
        if self.spreading_factor < 5 || self.spreading_factor > 12 {
            return Err(ProfileError::SpreadingFactor);
        }
        if self.coding_rate_denominator < 5 || self.coding_rate_denominator > 8 {
            return Err(ProfileError::CodingRate);
        }
        if self.preamble_symbols == 0 {
            return Err(ProfileError::Preamble);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    Command,
    Length,
    Frequency,
    Bandwidth,
    SpreadingFactor,
    CodingRate,
    Preamble,
}

/// Encode a complete runtime radio-profile command.
pub const fn encode_config_command(
    profile: PhyProfile,
) -> Result<[u8; CONFIG_COMMAND_LEN], ProfileError> {
    let profile = match profile.validate() {
        Ok(profile) => profile,
        Err(error) => return Err(error),
    };
    let mut out = [0_u8; CONFIG_COMMAND_LEN];
    out[0] = CMD_CONFIG;
    let frequency = profile.frequency_hz.to_le_bytes();
    out[1] = frequency[0];
    out[2] = frequency[1];
    out[3] = frequency[2];
    out[4] = frequency[3];
    let bandwidth = profile.bandwidth_hz.to_le_bytes();
    out[5] = bandwidth[0];
    out[6] = bandwidth[1];
    out[7] = bandwidth[2];
    out[8] = bandwidth[3];
    out[9] = profile.spreading_factor;
    out[10] = profile.coding_rate_denominator;
    let preamble = profile.preamble_symbols.to_le_bytes();
    out[11] = preamble[0];
    out[12] = preamble[1];
    out[13] = profile.sync_word;
    out[14] = (profile.explicit_header as u8)
        | ((profile.crc as u8) << 1)
        | ((profile.invert_iq as u8) << 2);
    out[15] = profile.tx_power_dbm as u8;
    Ok(out)
}

/// Decode and validate a complete runtime radio-profile command.
pub fn decode_config_command(command: &[u8]) -> Result<PhyProfile, ProfileError> {
    if command.len() != CONFIG_COMMAND_LEN {
        return Err(ProfileError::Length);
    }
    if command[0] != CMD_CONFIG {
        return Err(ProfileError::Command);
    }
    PhyProfile {
        frequency_hz: u32::from_le_bytes([command[1], command[2], command[3], command[4]]),
        bandwidth_hz: u32::from_le_bytes([command[5], command[6], command[7], command[8]]),
        spreading_factor: command[9],
        coding_rate_denominator: command[10],
        preamble_symbols: u16::from_le_bytes([command[11], command[12]]),
        sync_word: command[13],
        explicit_header: command[14] & 1 != 0,
        crc: command[14] & 2 != 0,
        invert_iq: command[14] & 4 != 0,
        tx_power_dbm: command[15] as i8,
    }
    .validate()
}

/// Convert the canonical one-byte LoRa sync word to the SX126x register form.
pub const fn sx126x_sync_word(sync_word: u8) -> [u8; 2] {
    [(sync_word & 0xf0) | 0x04, ((sync_word & 0x0f) << 4) | 0x04]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sync_words_have_the_documented_sx126x_encoding() {
        assert_eq!(sx126x_sync_word(0x34), [0x34, 0x44]);
        assert_eq!(sx126x_sync_word(0x12), [0x14, 0x24]);
        assert_eq!(sx126x_sync_word(MESHTASTIC_SYNC_WORD), [0x24, 0xb4]);
    }

    #[test]
    fn long_fast_keeps_frequency_a_board_setting() {
        let profile = PhyProfile::meshtastic_long_fast(906_875_000);
        assert_eq!(profile.frequency_hz, 906_875_000);
        assert_eq!(profile.sync_word, MESHTASTIC_SYNC_WORD);
        assert_eq!(profile.preamble_symbols, 16);
    }

    #[test]
    fn meshcore_profile_tracks_runtime_radio_settings_and_preamble_rule() {
        let slow = PhyProfile::meshcore(915_000_000, 250_000, 10, 5);
        assert_eq!(slow.sync_word, MESHCORE_SYNC_WORD);
        assert_eq!(slow.preamble_symbols, 16);
        assert!(slow.crc);

        let fast = PhyProfile::meshcore(915_000_000, 62_500, 8, 5);
        assert_eq!(fast.preamble_symbols, 32);
    }

    #[test]
    fn runtime_config_round_trips_all_profile_fields() {
        let mut profile = PhyProfile::meshtastic_long_fast(906_875_000);
        profile.sync_word = 0x12;
        profile.invert_iq = true;
        profile.tx_power_dbm = 11;
        let command = encode_config_command(profile).unwrap();
        assert_eq!(command[0], CMD_CONFIG);
        assert_eq!(decode_config_command(&command), Ok(profile));
    }

    #[test]
    fn runtime_config_rejects_invalid_profiles() {
        let mut profile = PhyProfile::meshtastic_long_fast(906_875_000);
        profile.spreading_factor = 13;
        assert_eq!(
            encode_config_command(profile),
            Err(ProfileError::SpreadingFactor)
        );
    }

    #[test]
    fn command_stream_reassembles_snapshot_at_every_byte_boundary() {
        let payload = [1, 15, 0, 0, 0x55, 0xaa];
        let mut wire = [0_u8; MAX_UI_SNAPSHOT_COMMAND_LEN];
        let wire_len = encode_ui_snapshot_command(&payload, &mut wire).unwrap();

        let mut stream = CommandStream::new();
        let mut command = [0_u8; MAX_COMMAND_LEN];
        let mut event = CommandEvent::Pending;
        for byte in wire[..wire_len].iter().copied() {
            event = stream.push(byte, &mut command);
        }
        assert_eq!(
            event,
            CommandEvent::Complete {
                kind: CommandKind::UiSnapshot,
                len: wire_len - 1,
            }
        );
        assert_eq!(&command[..wire_len - 1], &wire[..wire_len - 1]);
        let mut decoded = [0_u8; MAX_UI_SNAPSHOT_LEN];
        let decoded_len =
            decode_ui_snapshot_command(&command[..wire_len - 1], &mut decoded).unwrap();
        assert_eq!(&decoded[..decoded_len], &payload);
        assert!(stream.is_boundary());
    }

    #[test]
    fn rejected_oversized_snapshot_does_not_consume_following_config() {
        let declared = MAX_UI_SNAPSHOT_LEN + 1;
        let profile = PhyProfile::meshtastic_long_fast(906_875_000);
        let config = encode_config_command(profile).unwrap();
        let mut stream = CommandStream::new();
        let mut command = [0_u8; MAX_COMMAND_LEN];
        let mut events = [CommandEvent::Pending; 2];
        let mut count = 0;

        for byte in core::iter::once(CMD_UI_SNAPSHOT)
            .chain(core::iter::repeat_n(b'a', declared * 2))
            .chain(core::iter::once(WAKE_BYTE))
            .chain(config)
        {
            let event = stream.push(byte, &mut command);
            if event != CommandEvent::Pending {
                events[count] = event;
                count += 1;
            }
        }

        assert_eq!(
            &events[..count],
            &[
                CommandEvent::TooLong {
                    kind: CommandKind::UiSnapshot,
                    declared,
                },
                CommandEvent::Complete {
                    kind: CommandKind::Configure,
                    len: CONFIG_COMMAND_LEN,
                },
            ]
        );
        assert_eq!(&command[..CONFIG_COMMAND_LEN], &config);
    }

    #[test]
    fn truncated_snapshot_ends_at_next_wake_without_consuming_config() {
        let mut wire = [0_u8; MAX_UI_SNAPSHOT_COMMAND_LEN];
        let wire_len = encode_ui_snapshot_command(&[1, 2, 3], &mut wire).unwrap();
        let profile = PhyProfile::meshtastic_long_fast(906_875_000);
        let config = encode_config_command(profile).unwrap();
        let mut stream = CommandStream::new();
        let mut command = [0_u8; MAX_COMMAND_LEN];
        let mut saw_truncated = false;
        let mut saw_config = false;

        for byte in wire[..wire_len - 2]
            .iter()
            .copied()
            .chain(core::iter::once(WAKE_BYTE))
            .chain(config)
        {
            match stream.push(byte, &mut command) {
                CommandEvent::Complete {
                    kind: CommandKind::UiSnapshot,
                    len,
                } => {
                    let mut decoded = [0_u8; MAX_UI_SNAPSHOT_LEN];
                    assert_eq!(
                        decode_ui_snapshot_command(&command[..len], &mut decoded),
                        Err(UiSnapshotWireError::OddLength)
                    );
                    saw_truncated = true;
                }
                CommandEvent::Complete {
                    kind: CommandKind::Configure,
                    len,
                } => {
                    assert_eq!(len, CONFIG_COMMAND_LEN);
                    assert_eq!(&command[..len], &config);
                    saw_config = true;
                }
                CommandEvent::Pending => {}
                other => panic!("unexpected command event {other:?}"),
            }
        }

        assert!(saw_truncated);
        assert!(saw_config);
    }

    #[test]
    fn wake_prefix_is_ignored_only_at_a_command_boundary() {
        let mut stream = CommandStream::new();
        let mut command = [0_u8; MAX_COMMAND_LEN];
        for _ in 0..8 {
            assert_eq!(stream.push(WAKE_BYTE, &mut command), CommandEvent::Pending);
        }
        let bytes = [CMD_TX, 1, 0, WAKE_BYTE];
        let mut event = CommandEvent::Pending;
        for byte in bytes {
            event = stream.push(byte, &mut command);
        }
        assert_eq!(
            event,
            CommandEvent::Complete {
                kind: CommandKind::Transmit,
                len: bytes.len(),
            }
        );
        assert_eq!(&command[..bytes.len()], &bytes);
    }
}
