//! The zero-delimited, hex-bodied UI-snapshot command.

use crate::{CMD_UI_SNAPSHOT, MAX_UI_SNAPSHOT_COMMAND_LEN, MAX_UI_SNAPSHOT_LEN, WAKE_BYTE};

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
/// removed by [`CommandStream`](crate::CommandStream).
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
    for (index, pair) in encoded.as_chunks::<2>().0.iter().enumerate() {
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
