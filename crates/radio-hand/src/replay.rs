//! Replaying the desktop fixture corpus on a board.
//!
//! Gate N3's done condition asks that `ingest` and `poll` "replayed against the desktop
//! fixture corpus produce identical Actions". Identical to what the desktop produces — which
//! is the whole claim of an executor-neutral core, and is worth nothing as an assertion. So
//! this is the machinery for checking it: one byte form for a set of [`Actions`], and one
//! fixed identity both sides build a node from.
//!
//! # Why a separate feature from `node`
//!
//! Nothing here touches a radio, so nothing here needs `lora-phy`, whose ungated `defmt`
//! wants a global logger only a firmware provides. Keeping the codec out from under that is
//! what lets the desktop test link it and assert the expected bytes, which is the half of the
//! comparison that can live in CI. The board half is a hardware receipt, like every other RF
//! claim in this plan.
//!
//! # What makes the comparison meaningful
//!
//! Every input to the node is pinned. The identity is [`REPLAY_SEED`] rather than the
//! board's own, so both sides are the same node; the clock is passed in rather than read, so
//! neither side depends on when it ran; and `poll`'s entropy is caller-supplied, which the
//! protocol layer already required for exactly this reason. What remains is the protocol's
//! own decisions, which is the only thing the comparison is about.

extern crate alloc;

use alloc::vec::Vec;

use retinue::destination::DestinationName;
use retinue::hash::{ADDRESS_HASH_LEN, AddressHash, NameHash};
use retinue::identity::PrivateIdentity;
use retinue::node::{Action, Actions, Node};

/// The identity a replay node is built from, on the board and on the desk alike.
///
/// A test key, never the board's own: the board's identity stays in flash and is not what
/// this compares. `0x11` repeated is the same filler `retinue`'s own node tests use for their
/// first node, so a replay node is a node those tests already describe.
pub const REPLAY_SEED: [u8; 64] = [0x11; 64];

/// The destination a replay node announces, matching the board's real one so a replayed
/// announce is the announce the board would actually send.
pub fn replay_name_hash() -> NameHash {
    DestinationName::new("retinue", ["node"]).name_hash()
}

/// A node for replaying fixtures.
///
/// Generic over the capacities so a board and a desk instantiate the same profile rather
/// than two that happen to agree today: a capacity bound that changes behaviour has to
/// change it identically on both sides, or the comparison is measuring the wrong thing.
pub fn replay_node<const PEERS: usize, const ACTIONS: usize, const LINKS: usize>()
-> Node<PEERS, ACTIONS, LINKS> {
    Node::new(
        PrivateIdentity::from_secret_bytes(&REPLAY_SEED),
        replay_name_hash(),
    )
}

/// The encoding's version, first byte of every block. Bumped if the shape below changes, so
/// a board and a desk that disagree say so instead of comparing nonsense.
pub const VERSION: u8 = 1;

pub const ACTION_SEND: u8 = 0x01;
pub const ACTION_LEARNED: u8 = 0x02;
pub const ACTION_LINK_UP: u8 = 0x03;
pub const ACTION_LINK_DOWN: u8 = 0x04;
pub const ACTION_DATA: u8 = 0x05;
pub const ACTION_RESOURCE: u8 = 0x06;

/// Encode a set of actions.
///
/// ```text
/// version u8 | count u8 | overflowed u16le | action*
///
/// Send      0x01 | interface u32le | len u16le | packet
/// Learned   0x02 | destination [16]
/// LinkUp    0x03 | link [16]
/// LinkDown  0x04 | link [16]
/// Data      0x05 | link [16] | len u16le | payload
/// Resource  0x06 | link [16] | len u16le | data
/// ```
///
/// Order is preserved, because the order a shell is asked to do things in is part of what
/// the two sides must agree on: a proof emitted before its data is not the same behaviour as
/// one emitted after.
pub fn encode_actions<const N: usize>(actions: &Actions<N>) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(VERSION);
    out.push(actions.len() as u8);
    out.extend_from_slice(&actions.overflowed().to_le_bytes());

    for action in actions.iter() {
        match action {
            Action::Send { interface, packet } => {
                out.push(ACTION_SEND);
                out.extend_from_slice(&interface.to_le_bytes());
                let bytes = packet.encode();
                out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                out.extend_from_slice(&bytes);
            }
            Action::Learned { destination } => {
                out.push(ACTION_LEARNED);
                push_hash(&mut out, destination);
            }
            Action::LinkUp { link_id } => {
                out.push(ACTION_LINK_UP);
                push_hash(&mut out, link_id);
            }
            Action::LinkDown { link_id } => {
                out.push(ACTION_LINK_DOWN);
                push_hash(&mut out, link_id);
            }
            Action::Data { link_id, payload } => {
                out.push(ACTION_DATA);
                push_hash(&mut out, link_id);
                out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
                out.extend_from_slice(payload);
            }
            Action::Resource { link_id, data } => {
                out.push(ACTION_RESOURCE);
                push_hash(&mut out, link_id);
                out.extend_from_slice(&(data.len() as u16).to_le_bytes());
                out.extend_from_slice(data);
            }
        }
    }
    out
}

/// The encoding of a set with no actions.
///
/// What a frame that is not a packet produces. The two sides reach it by different routes —
/// the board refuses the frame before ingest, the desk refuses it before building a node —
/// so having one name for "nothing happened" is what keeps them comparing the same thing.
pub fn encode_nothing() -> Vec<u8> {
    alloc::vec![VERSION, 0, 0, 0]
}

fn push_hash(out: &mut Vec<u8>, hash: &AddressHash) {
    debug_assert_eq!(hash.as_slice().len(), ADDRESS_HASH_LEN);
    out.extend_from_slice(hash.as_slice());
}

/// Write `bytes` as lowercase hex into `out`, returning how many bytes were written.
///
/// The replay command and its reply are text lines, so a packet has to survive being carried
/// as one. Hex rather than base64 because the reply is read by a human as often as by a
/// script, and a wrong nibble is easier to find.
pub fn to_hex(bytes: &[u8], out: &mut [u8]) -> usize {
    let mut at = 0;
    for byte in bytes {
        if at + 2 > out.len() {
            break;
        }
        out[at] = hex_digit(byte >> 4);
        out[at + 1] = hex_digit(byte & 0x0f);
        at += 2;
    }
    at
}

/// Read lowercase or uppercase hex into `out`, returning how many bytes were read.
pub fn from_hex(text: &[u8], out: &mut [u8]) -> Option<usize> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut at = 0;
    for pair in text.chunks(2) {
        if at >= out.len() {
            return None;
        }
        out[at] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        at += 1;
    }
    Some(at)
}

fn hex_digit(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'a' + (nibble - 10),
    }
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = [0x00, 0x1f, 0xa5, 0xff, 0x10];
        let mut text = [0_u8; 10];
        let written = to_hex(&bytes, &mut text);
        assert_eq!(&text[..written], b"001fa5ff10");

        let mut back = [0_u8; 5];
        assert_eq!(from_hex(&text[..written], &mut back), Some(5));
        assert_eq!(back, bytes);
    }

    #[test]
    fn odd_length_hex_is_refused() {
        let mut out = [0_u8; 4];
        assert_eq!(from_hex(b"abc", &mut out), None);
    }

    #[test]
    fn a_non_hex_digit_is_refused() {
        let mut out = [0_u8; 4];
        assert_eq!(from_hex(b"ab!f", &mut out), None);
    }

    /// A replay node is the same node wherever it is built. If this value moves, every
    /// committed expectation below moves with it, which is the point of pinning it.
    #[test]
    fn the_replay_identity_is_fixed() {
        let mut text = [0_u8; 32];
        let written = to_hex(
            replay_node::<32, 8, 4>().destination().as_slice(),
            &mut text,
        );
        assert_eq!(
            core::str::from_utf8(&text[..written]).unwrap(),
            "185efd55eca87398a50cdc3a78979a8b"
        );
    }
}
