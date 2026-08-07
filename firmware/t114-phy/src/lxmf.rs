//! The board's own account of whether it can read LXMF.
//!
//! Linking outrider is not the same as running it. This is the difference, asserted on the
//! hardware: a stock LXMF 0.9.6 message captured from the pinned oracle, baked into flash,
//! decoded by the board's own CPU on the board's own 48 KB heap, and checked against the
//! message id the oracle gave it. A host comparing the two would prove the same thing, but
//! only while a host is attached; this way the board knows, and says so when it is wrong.
//!
//! The stamp half is the more interesting one. Scoring a propagation stamp the materialised
//! way needs 256 KB of workblock, which this board does not have and never will, so the
//! probe exercises the streamed path and reports what it cost in time. That number is the
//! answer to whether a board can weigh inbound proof-of-work at all.

use core::fmt::Write as _;

use embassy_time::Instant;
use outrider::portable;
use outrider::stamp;

use crate::heap;

/// A stock LXMF 0.9.6 message, captured 2026-07-27 from the pinned oracle.
const MESSAGE: [u8; 127] = [
    0x83, 0xe3, 0x17, 0x12, 0x10, 0xb2, 0x5a, 0xfd, 0x74, 0x89, 0x20, 0xa7,
    0xd1, 0xd8, 0x32, 0xb5, 0x66, 0xa3, 0x28, 0xc5, 0xb5, 0x50, 0xde, 0xf7,
    0x25, 0x21, 0x5a, 0xca, 0xc8, 0xf5, 0xc1, 0x3b, 0xfa, 0xd9, 0xea, 0x55,
    0xeb, 0xb1, 0x76, 0x71, 0x94, 0x9f, 0x52, 0xbd, 0xb7, 0x30, 0x07, 0xf9,
    0xa9, 0x58, 0x47, 0x21, 0x54, 0x79, 0xa1, 0x69, 0x05, 0x9d, 0x3c, 0x9b,
    0x2c, 0x3c, 0x55, 0xbd, 0xa1, 0x55, 0x5d, 0x8a, 0xcc, 0x6d, 0x7a, 0x65,
    0x0d, 0xe7, 0xb8, 0xaa, 0xe8, 0x5f, 0x97, 0x8d, 0xd1, 0x2a, 0x87, 0xf3,
    0xcd, 0xf0, 0x02, 0x40, 0x82, 0x7c, 0xf2, 0x2c, 0xb0, 0xcd, 0xad, 0x04,
    0x94, 0xcb, 0x41, 0xda, 0x21, 0x77, 0x20, 0x20, 0x00, 0x00, 0xc4, 0x05,
    0x54, 0x49, 0x54, 0x4c, 0x45, 0xc4, 0x04, 0x42, 0x4f, 0x44, 0x59, 0x81,
    0x07, 0xc4, 0x04, 0x6d, 0x65, 0x74, 0x61,
];

/// What the oracle calls that message.
const EXPECTED_ID: [u8; 32] = [
    0x9d, 0x0b, 0x23, 0x7d, 0xb6, 0xf9, 0x18, 0x6e, 0xf3, 0x96, 0x3d, 0x8a,
    0x7e, 0xc7, 0xe1, 0xfc, 0x1c, 0x56, 0x9c, 0x2f, 0x3b, 0xf4, 0x16, 0x0d,
    0x5b, 0xbb, 0xc3, 0xd5, 0x2a, 0x50, 0x8c, 0xf6,
];

/// A captured propagation transient id and the stamp minted for it, which stock LXMF scores
/// at 14 leading zero bits.
const PROPAGATION_ID: [u8; 32] = [
    0x51, 0x10, 0x62, 0xe6, 0x86, 0x83, 0x1f, 0xdd, 0xd0, 0x10, 0x61, 0x40,
    0x1b, 0x86, 0xc6, 0x9d, 0x1e, 0x7d, 0x67, 0x25, 0x95, 0xbe, 0x43, 0xa5,
    0x4a, 0x70, 0x46, 0xf1, 0xc1, 0x18, 0x69, 0x8c,
];
const PROPAGATION_STAMP: [u8; 32] = [
    0x01, 0x67, 0x82, 0xee, 0x98, 0x40, 0x65, 0x98, 0x31, 0x8e, 0xb0, 0xbc,
    0x18, 0xa5, 0x06, 0x5b, 0x2f, 0x6d, 0x64, 0xe8, 0xac, 0x41, 0x9a, 0x1a,
    0x30, 0xc8, 0x16, 0x18, 0x0e, 0x4d, 0xa7, 0xe5,
];
const EXPECTED_PROPAGATION_VALUE: u16 = 14;

/// Decode the baked-in message and check it against the oracle's answer.
///
/// Reports the heap the decode actually took, which is the figure that decides whether a
/// message lane fits beside everything else the board is already holding.
pub fn check_codec(reply: &mut radio_face::Text<256>) {
    let before = heap::used();
    let started = Instant::now();
    let decoded = portable::decode(&MESSAGE);
    let took = started.elapsed().as_micros();
    let cost = heap::used().saturating_sub(before);

    match decoded {
        Err(error) => {
            let _ = write!(reply, "lxmf codec FAILED {error}\r\n");
        }
        Ok(message) if message.message_id != EXPECTED_ID => {
            // Loud and attributable: the id the board computed, so a divergence can be
            // chased rather than merely noticed.
            let _ = write!(reply, "lxmf codec MISMATCH id=");
            for byte in &message.message_id[..8] {
                let _ = write!(reply, "{byte:02x}");
            }
            let _ = write!(reply, " want=");
            for byte in &EXPECTED_ID[..8] {
                let _ = write!(reply, "{byte:02x}");
            }
            let _ = write!(reply, "\r\n");
        }
        Ok(message) => {
            let _ = write!(
                reply,
                "lxmf codec ok title={} content={} fields={} took={took}us heap={cost}\r\n",
                message.payload.title.len(),
                message.payload.content.len(),
                message.payload.fields.len(),
            );
        }
    }
}

/// Score the captured propagation stamp the streamed way, and time it.
///
/// A thousand rounds of HKDF-SHA256 on a 64 MHz Cortex-M4 is real work, so the elapsed
/// figure is as much the point of the probe as the score is.
pub fn check_stamp(reply: &mut radio_face::Text<256>) {
    let before = heap::used();
    let started = Instant::now();
    let scored = stamp::value_streamed(
        &PROPAGATION_ID,
        stamp::PROPAGATION_WORKBLOCK_ROUNDS,
        &PROPAGATION_STAMP,
    );
    let took = started.elapsed().as_millis();
    let cost = heap::used().saturating_sub(before);

    if scored == EXPECTED_PROPAGATION_VALUE {
        let _ = write!(
            reply,
            "lxmf stamp ok value={scored} rounds={} took={took}ms heap={cost}\r\n",
            stamp::PROPAGATION_WORKBLOCK_ROUNDS,
        );
    } else {
        let _ = write!(
            reply,
            "lxmf stamp MISMATCH value={scored} want={EXPECTED_PROPAGATION_VALUE} took={took}ms\r\n",
        );
    }
}
