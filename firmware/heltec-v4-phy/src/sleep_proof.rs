//! The RF sleep-proof wire format, for the `rf-sleep-proof` bench build.
//!
//! The low-power board's UART host is intentionally absent from that bench, so a
//! feature-gated RF challenge returns the sleep counters through the attached T114. The
//! formats live here so `main` carries only the machinery that uses them.

const CHALLENGE: &[u8; 21] = b"tulle/sleep-proof/v1?";
const RECEIPT_MARKER: &[u8; 21] = b"tulle/sleep-proof/v1!";

/// The nonce carried by a challenge frame, if `frame` is one.
pub fn nonce(frame: &[u8]) -> Option<u32> {
    let nonce = frame.strip_prefix(CHALLENGE)?;
    let nonce: [u8; 4] = nonce.try_into().ok()?;
    Some(u32::from_le_bytes(nonce))
}

/// Build the receipt frame answering `nonce`.
#[allow(clippy::too_many_arguments)]
pub fn receipt(
    nonce: u32,
    sleep_entries: u32,
    wake_registrations: u32,
    received_frames: u32,
    last_sleep_us: u32,
    sleep_enabled: bool,
    reset_reason: u32,
) -> [u8; 49] {
    let mut receipt = [0_u8; 49];
    receipt[..RECEIPT_MARKER.len()].copy_from_slice(RECEIPT_MARKER);
    receipt[21..25].copy_from_slice(&nonce.to_le_bytes());
    receipt[25..29].copy_from_slice(&sleep_entries.to_le_bytes());
    receipt[29..33].copy_from_slice(&wake_registrations.to_le_bytes());
    receipt[33..37].copy_from_slice(&received_frames.to_le_bytes());
    receipt[37..41].copy_from_slice(&last_sleep_us.to_le_bytes());
    receipt[41..45].copy_from_slice(&u32::from(sleep_enabled).to_le_bytes());
    receipt[45..49].copy_from_slice(&reset_reason.to_le_bytes());
    receipt
}
