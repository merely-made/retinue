//! LXMF tickets: a 16-byte secret one peer hands another, so its replies can carry
//! `truncated_hash(ticket || message_id)` in place of a proof-of-work stamp.
//!
//! Everything here is `no_std`: the ticket stamp, the stamp check that tries tickets before
//! proof of work, and the `FIELD_TICKET` entry read from and written into a raw field map.
//! The policy that issues, learns and expires tickets is [`TicketBook`], which needs a host.

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use crate::portable::CodecError;
use crate::portable::msgpack::{read_array_len, skip};
use crate::stamp::{MESSAGE_WORKBLOCK_ROUNDS, STAMP_LEN, value_streamed};

#[cfg(feature = "std")]
mod book;
#[cfg(feature = "std")]
pub use book::{TicketBook, TicketBookError, set_ticket_field, ticket_field};

pub const TICKET_LEN: usize = 16;
/// The field key a ticket travels under: `[expires, ticket]`.
pub const FIELD_TICKET: u8 = 0x0C;
/// The stamp value a ticket stamp is credited with: above any proof of work.
pub const COST_TICKET: u16 = 0x100;

const DAY: f64 = 24.0 * 60.0 * 60.0;
/// How long an issued ticket is valid.
pub const TICKET_EXPIRY: f64 = 21.0 * DAY;
/// An issued ticket with less validity than this left is replaced rather than reissued.
pub const TICKET_RENEW: f64 = 14.0 * DAY;
/// At most one ticket is delivered to a destination per interval.
pub const TICKET_INTERVAL: f64 = DAY;
/// Expired inbound tickets are kept this long before cleanup, for clock skew. They no
/// longer validate stamps.
pub const TICKET_GRACE: f64 = 5.0 * DAY;

/// The stamp a ticket holder puts on a message: the first 16 bytes of
/// `SHA-256(ticket || message_id)`.
pub fn ticket_stamp(ticket: &[u8; TICKET_LEN], message_id: &[u8; 32]) -> [u8; TICKET_LEN] {
    let digest = Sha256::new()
        .chain_update(ticket)
        .chain_update(message_id)
        .finalize();
    digest[..TICKET_LEN].try_into().unwrap()
}

/// Whether `stamp` is in the shape of a ticket stamp. Only a receiver holding the ticket
/// can tell whether it is a valid one.
pub fn is_ticket_stamp(stamp: Option<&[u8]>) -> bool {
    stamp.is_some_and(|stamp| stamp.len() == TICKET_LEN)
}

/// How a stamp met its cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampOutcome {
    /// It matched a ticket this receiver issued to the sender.
    Ticket,
    /// Proof of work of this value.
    Work(u16),
}

impl StampOutcome {
    /// The stamp's value, with a ticket counted as [`COST_TICKET`].
    pub fn value(self) -> u16 {
        match self {
            Self::Ticket => COST_TICKET,
            Self::Work(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampFault {
    Missing,
    Invalid,
}

/// Check a received stamp against `target`: first against the tickets issued to its sender,
/// then as proof of work.
pub fn check_stamp(
    message_id: &[u8; 32],
    stamp: Option<&[u8]>,
    target: u8,
    tickets: &[[u8; TICKET_LEN]],
) -> Result<StampOutcome, StampFault> {
    let stamp = stamp.ok_or(StampFault::Missing)?;
    if tickets
        .iter()
        .any(|ticket| stamp == ticket_stamp(ticket, message_id))
    {
        return Ok(StampOutcome::Ticket);
    }
    let work = <&[u8; STAMP_LEN]>::try_from(stamp).map_err(|_| StampFault::Invalid)?;
    let value = value_streamed(message_id, MESSAGE_WORKBLOCK_ROUNDS, work);
    if value >= u16::from(target) {
        Ok(StampOutcome::Work(value))
    } else {
        Err(StampFault::Invalid)
    }
}

/// A ticket as it travels in `FIELD_TICKET`: `[expires, ticket]`, expiry in Unix seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ticket {
    pub expires: f64,
    pub ticket: [u8; TICKET_LEN],
}

impl Ticket {
    pub fn is_valid_at(&self, now: f64) -> bool {
        now < self.expires
    }

    /// The MessagePack field value: a two-item array of a double and a 16-byte binary.
    pub fn encode(&self) -> [u8; 28] {
        let mut out = [0; 28];
        out[..2].copy_from_slice(&[0x92, 0xcb]);
        out[2..10].copy_from_slice(&self.expires.to_be_bytes());
        out[10..12].copy_from_slice(&[0xc4, TICKET_LEN as u8]);
        out[12..].copy_from_slice(&self.ticket);
        out
    }

    /// Read a field value. Like stock, extra trailing items are ignored, and the expiry
    /// may be any number.
    pub fn decode(value: &[u8]) -> Option<Self> {
        let mut at = 0;
        let items = read_array_len(value, &mut at).ok()?;
        if items < 2 {
            return None;
        }
        let expires = read_number(value, &mut at)?;
        let ticket = match value.get(at..at + 2 + TICKET_LEN)? {
            [0xc4, len, ticket @ ..] if usize::from(*len) == TICKET_LEN => ticket,
            _ => return None,
        };
        Some(Self {
            expires,
            ticket: ticket.try_into().ok()?,
        })
    }

    /// Find the ticket in a raw MessagePack field map. `None` when absent or malformed.
    pub fn from_fields(fields: &[u8]) -> Option<Self> {
        Entries::new(fields)
            .ok()?
            .map_while(Result::ok)
            .find(|entry| entry.key == Some(u64::from(FIELD_TICKET)))
            .and_then(|entry| Self::decode(entry.value))
    }

    /// Put this ticket into a raw field map, replacing a ticket already there in place and
    /// otherwise appending it, as a dictionary insert would.
    pub fn insert_into(&self, fields: &[u8]) -> Result<Vec<u8>, CodecError> {
        let mut entries = Entries::new(fields)?;
        let mut body = Vec::with_capacity(fields.len() + 30);
        let mut count = 0_usize;
        let mut replaced = false;
        for entry in entries.by_ref() {
            let entry = entry?;
            if entry.key == Some(u64::from(FIELD_TICKET)) && !replaced {
                body.push(FIELD_TICKET);
                body.extend_from_slice(&self.encode());
                replaced = true;
            } else {
                body.extend_from_slice(entry.raw);
            }
            count += 1;
        }
        if entries.at != fields.len() {
            return Err(CodecError::MalformedMessagePack);
        }
        if !replaced {
            body.push(FIELD_TICKET);
            body.extend_from_slice(&self.encode());
            count += 1;
        }
        let mut out = Vec::with_capacity(body.len() + 5);
        match count {
            0..=15 => out.push(0x80 | count as u8),
            16..=0xffff => {
                out.push(0xde);
                out.extend_from_slice(&(count as u16).to_be_bytes());
            }
            _ => {
                out.push(0xdf);
                out.extend_from_slice(&(count as u32).to_be_bytes());
            }
        }
        out.extend_from_slice(&body);
        Ok(out)
    }
}

/// One entry of a raw field map: its integer key, if it has one, its value, and both.
struct Entry<'a> {
    key: Option<u64>,
    value: &'a [u8],
    raw: &'a [u8],
}

/// The entries of a raw MessagePack map, walked without decoding the values.
struct Entries<'a> {
    bytes: &'a [u8],
    at: usize,
    left: usize,
}

impl<'a> Entries<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, CodecError> {
        let (left, at) = match *bytes.first().ok_or(CodecError::InvalidFields)? {
            marker @ 0x80..=0x8f => (usize::from(marker & 0x0f), 1),
            0xde => (
                be(bytes.get(1..3).ok_or(CodecError::InvalidFields)?) as usize,
                3,
            ),
            0xdf => (
                be(bytes.get(1..5).ok_or(CodecError::InvalidFields)?) as usize,
                5,
            ),
            _ => return Err(CodecError::InvalidFields),
        };
        Ok(Self { bytes, at, left })
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = Result<Entry<'a>, CodecError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let start = self.at;
        let walked = skip(self.bytes, &mut self.at).and_then(|()| {
            let value_start = self.at;
            skip(self.bytes, &mut self.at).map(|()| value_start)
        });
        match walked {
            Ok(value_start) => Some(Ok(Entry {
                key: read_uint(&self.bytes[start..value_start]),
                value: &self.bytes[value_start..self.at],
                raw: &self.bytes[start..self.at],
            })),
            Err(error) => {
                self.left = 0;
                Some(Err(error))
            }
        }
    }
}

fn be(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, b| (value << 8) | u64::from(*b))
}

/// A complete MessagePack unsigned integer, in any width.
fn read_uint(bytes: &[u8]) -> Option<u64> {
    match bytes {
        [value @ 0..=0x7f] => Some(u64::from(*value)),
        [0xcc, rest @ ..] if rest.len() == 1 => Some(be(rest)),
        [0xcd, rest @ ..] if rest.len() == 2 => Some(be(rest)),
        [0xce, rest @ ..] if rest.len() == 4 => Some(be(rest)),
        [0xcf, rest @ ..] if rest.len() == 8 => Some(be(rest)),
        _ => None,
    }
}

/// A MessagePack number as a double: what stock compares a ticket expiry with.
fn read_number(bytes: &[u8], at: &mut usize) -> Option<f64> {
    let start = *at;
    skip(bytes, at).ok()?;
    let raw = &bytes[start..*at];
    match raw {
        [0xcb, rest @ ..] => Some(f64::from_be_bytes(rest.try_into().ok()?)),
        [0xca, rest @ ..] => Some(f64::from(f32::from_be_bytes(rest.try_into().ok()?))),
        [value @ 0xe0..=0xff] => Some(f64::from(*value as i8)),
        [0xd0..=0xd3, rest @ ..] => {
            let shift = 64 - 8 * rest.len() as u32;
            Some(((be(rest) as i64) << shift >> shift) as f64)
        }
        _ => read_uint(raw).map(|value| value as f64),
    }
}

#[cfg(test)]
mod tests;
