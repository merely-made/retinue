//! Direct-PHY wire markers, bounds and result codes.

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
/// Request one bounded, board-timed alternate PHY interval.
pub const CMD_EXCURSION: u8 = 0x06;

/// Direct-PHY firmware-to-host event markers.
pub const EVENT_RX: u8 = 0x81;
pub const EVENT_TX: u8 = 0x82;
pub const EVENT_CONFIG: u8 = 0x83;
/// Firmware-to-host SX126x diagnostic event marker.
pub const EVENT_DIAGNOSTIC: u8 = 0x84;
/// Result of a [`CMD_UI_SNAPSHOT`] command.
pub const EVENT_UI_SNAPSHOT: u8 = 0x85;
/// Board-timed excursion admission and restoration status.
pub const EVENT_EXCURSION: u8 = 0x87;

/// Bytes in a complete [`CMD_CONFIG`] command.
pub const CONFIG_COMMAND_LEN: usize = 16;
/// Bytes in a complete [`CMD_EXCURSION`] command: profile command plus LE duration ms.
pub const EXCURSION_COMMAND_LEN: usize = CONFIG_COMMAND_LEN + 8;
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

// Results of a [`CMD_TX`] command, carried by [`EVENT_TX`]. A host can see them,
// so they live with the other wire results rather than in a board.

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
/// The board has no region configured, so it does not transmit. A regulatory
/// refusal, not a fault: the board is waiting to be told where it is.
pub const TX_NO_REGION: u8 = 6;
/// The region's duty-cycle budget is exhausted; the frame was refused rather
/// than sent over the limit. Transmitting resumes as the window drains.
pub const TX_OVER_DUTY: u8 = 7;
/// The channel stayed busy for the whole listen-before-talk budget, so the frame
/// was not sent. Not a fault: the reliability layer above retries.
pub const TX_CHANNEL_BUSY: u8 = 8;

// Results of a [`CMD_CONFIG`] command, carried by [`EVENT_CONFIG`].

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
/// The profile's frequency falls outside the board's configured region, or no
/// region is configured. The profile is rejected whole; power, by contrast, is
/// clamped, and the status reports the applied value.
pub const CONFIG_OUT_OF_REGION: u8 = 4;

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
