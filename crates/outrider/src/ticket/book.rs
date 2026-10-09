//! The ticket policy: what a peer has issued, learned and delivered, as caller-persisted state.

use std::collections::BTreeMap;
use std::io::Cursor;

use retinue::hash::AddressHash;
use retinue::identity::Identity;
use rmpv::Value;

use super::{
    TICKET_EXPIRY, TICKET_GRACE, TICKET_INTERVAL, TICKET_LEN, TICKET_RENEW, Ticket, ticket_stamp,
};
use crate::announce::delivery_destination;
use crate::codec::{CodecError, DecodedLxmf, LxmfPayload, prepare};

const SNAPSHOT_VERSION: u64 = 1;

/// Tickets by peer `lxmf.delivery` destination, with the clock always supplied by the caller.
///
/// - **Outbound**: the ticket each peer gave us, used in place of proof of work on what we
///   send it.
/// - **Inbound**: the tickets we gave each peer, which validate the stamps on what it sends.
/// - **Issued**: when a message carrying our ticket last reached each peer, which limits
///   issuance to one per [`TICKET_INTERVAL`].
///
/// Persist [`Self::encode_snapshot`] after changes and [`Self::restore`] it on start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TicketBook {
    pub(super) outbound: BTreeMap<AddressHash, Ticket>,
    pub(super) inbound: BTreeMap<AddressHash, Vec<Ticket>>,
    pub(super) issued: BTreeMap<AddressHash, f64>,
}

impl TicketBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// A ticket to include in a message to `destination`, or `None` when one was delivered
    /// there within the last interval. An issued ticket with more than [`TICKET_RENEW`] left
    /// is reused; otherwise `fresh`, which must be random, becomes a new one.
    pub fn issue(
        &mut self,
        destination: AddressHash,
        now: f64,
        fresh: [u8; TICKET_LEN],
    ) -> Option<Ticket> {
        if self
            .issued
            .get(&destination)
            .is_some_and(|at| now - at < TICKET_INTERVAL)
        {
            return None;
        }
        let issued = self.inbound.entry(destination).or_default();
        if let Some(ticket) = issued.iter().find(|t| t.expires - now > TICKET_RENEW) {
            return Some(*ticket);
        }
        let ticket = Ticket {
            expires: now + TICKET_EXPIRY,
            ticket: fresh,
        };
        issued.push(ticket);
        Some(ticket)
    }

    /// [`Self::issue`] a ticket into `payload`'s fields. Do this before stamping: the field
    /// is part of the message id.
    pub fn include(
        &mut self,
        payload: &mut LxmfPayload,
        destination: AddressHash,
        now: f64,
        fresh: [u8; TICKET_LEN],
    ) -> Option<Ticket> {
        let ticket = self.issue(destination, now, fresh)?;
        set_ticket_field(&mut payload.fields, &ticket);
        Some(ticket)
    }

    /// Record that a message carrying our ticket was delivered to `destination`.
    pub fn delivered(&mut self, destination: AddressHash, now: f64) {
        self.issued.insert(destination, now);
    }

    /// Learn the ticket a received message carries, if it is unexpired and the message's
    /// signature verifies against `source`. The check is made here, so a message delivered
    /// unverified can never plant a ticket.
    pub fn learn(&mut self, message: &DecodedLxmf, source: &Identity, now: f64) -> Option<Ticket> {
        let ticket = ticket_field(&message.payload.fields).filter(|t| t.is_valid_at(now))?;
        let address = delivery_destination(source);
        if *address.as_bytes() != message.source
            || !message.verify_with(|bytes, signature| source.verify(bytes, signature))
        {
            return None;
        }
        self.outbound.insert(address, ticket);
        Some(ticket)
    }

    /// The unexpired ticket `destination` gave us.
    pub fn outbound(&self, destination: &AddressHash, now: f64) -> Option<Ticket> {
        self.outbound
            .get(destination)
            .filter(|t| t.is_valid_at(now))
            .copied()
    }

    /// Stamp `payload` with the ticket `destination` gave us, in place of proof of work.
    /// Returns whether a ticket was held. Include any ticket of our own first.
    pub fn stamp(
        &self,
        payload: &mut LxmfPayload,
        destination: AddressHash,
        sender: &Identity,
        now: f64,
    ) -> Result<bool, CodecError> {
        let Some(ticket) = self.outbound(&destination, now) else {
            return Ok(false);
        };
        payload.stamp = None;
        let source = delivery_destination(sender);
        let message_id = prepare(*destination.as_bytes(), *source.as_bytes(), payload)?.message_id;
        payload.stamp = Some(ticket_stamp(&ticket.ticket, &message_id).to_vec());
        Ok(true)
    }

    /// The unexpired tickets we gave `source`, which its stamps may be made with.
    pub fn inbound(&self, source: &AddressHash, now: f64) -> Vec<[u8; TICKET_LEN]> {
        self.inbound
            .get(source)
            .into_iter()
            .flatten()
            .filter(|t| t.is_valid_at(now))
            .map(|t| t.ticket)
            .collect()
    }

    /// Drop expired outbound tickets, inbound tickets past their grace, and delivery
    /// times old enough to no longer hold issuance back.
    pub fn clean(&mut self, now: f64) {
        self.outbound.retain(|_, t| t.is_valid_at(now));
        self.inbound.retain(|_, issued| {
            issued.retain(|t| now <= t.expires + TICKET_GRACE);
            !issued.is_empty()
        });
        self.issued.retain(|_, at| now - *at < TICKET_INTERVAL);
    }

    /// `[version, outbound, inbound, issued]`, each a list of `[destination, ...]` rows.
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, TicketBookError> {
        let row = |destination: &AddressHash, ticket: &Ticket| {
            Value::Array(vec![
                Value::Binary(destination.as_slice().to_vec()),
                Value::Binary(ticket.ticket.to_vec()),
                Value::F64(ticket.expires),
            ])
        };
        let outbound = self.outbound.iter().map(|(d, t)| row(d, t)).collect();
        let inbound = self
            .inbound
            .iter()
            .flat_map(|(d, issued)| issued.iter().map(move |t| row(d, t)))
            .collect();
        let issued = self
            .issued
            .iter()
            .map(|(d, at)| {
                Value::Array(vec![Value::Binary(d.as_slice().to_vec()), Value::F64(*at)])
            })
            .collect();
        let snapshot = Value::Array(vec![
            Value::from(SNAPSHOT_VERSION),
            Value::Array(outbound),
            Value::Array(inbound),
            Value::Array(issued),
        ]);
        let mut bytes = Vec::new();
        rmpv::encode::write_value(&mut bytes, &snapshot).map_err(|_| TicketBookError::Encode)?;
        Ok(bytes)
    }

    pub fn restore(snapshot: &[u8]) -> Result<Self, TicketBookError> {
        let mut cursor = Cursor::new(snapshot);
        let value = rmpv::decode::read_value(&mut cursor).map_err(|_| TicketBookError::Invalid)?;
        if cursor.position() as usize != snapshot.len() {
            return Err(TicketBookError::Invalid);
        }
        let Value::Array(parts) = value else {
            return Err(TicketBookError::Invalid);
        };
        let [version, outbound, inbound, issued] = parts.as_slice() else {
            return Err(TicketBookError::Invalid);
        };
        match version.as_u64() {
            Some(SNAPSHOT_VERSION) => {}
            Some(other) => return Err(TicketBookError::UnsupportedVersion(other)),
            None => return Err(TicketBookError::Invalid),
        }
        let mut book = Self::new();
        for row in rows(outbound)? {
            let (destination, ticket) = ticket_row(row)?;
            book.outbound.insert(destination, ticket);
        }
        for row in rows(inbound)? {
            let (destination, ticket) = ticket_row(row)?;
            book.inbound.entry(destination).or_default().push(ticket);
        }
        for row in rows(issued)? {
            let [destination, Value::F64(at)] = row else {
                return Err(TicketBookError::Invalid);
            };
            book.issued.insert(address(destination)?, *at);
        }
        Ok(book)
    }
}

fn rows(list: &Value) -> Result<impl Iterator<Item = &[Value]>, TicketBookError> {
    let Value::Array(rows) = list else {
        return Err(TicketBookError::Invalid);
    };
    Ok(rows.iter().map(|row| match row {
        Value::Array(items) => items.as_slice(),
        _ => &[],
    }))
}

fn ticket_row(row: &[Value]) -> Result<(AddressHash, Ticket), TicketBookError> {
    let [destination, Value::Binary(ticket), Value::F64(expires)] = row else {
        return Err(TicketBookError::Invalid);
    };
    let ticket = Ticket {
        expires: *expires,
        ticket: ticket
            .as_slice()
            .try_into()
            .map_err(|_| TicketBookError::Invalid)?,
    };
    Ok((address(destination)?, ticket))
}

fn address(value: &Value) -> Result<AddressHash, TicketBookError> {
    match value {
        Value::Binary(bytes) => <[u8; 16]>::try_from(bytes.as_slice())
            .map(AddressHash::from_bytes)
            .map_err(|_| TicketBookError::Invalid),
        _ => Err(TicketBookError::Invalid),
    }
}

/// The ticket in a decoded field map, if one is there and well formed.
pub fn ticket_field(fields: &Value) -> Option<Ticket> {
    let Value::Map(entries) = fields else {
        return None;
    };
    let (_, value) = entries
        .iter()
        .find(|(key, _)| key.as_u64() == Some(u64::from(super::FIELD_TICKET)))?;
    let Value::Array(items) = value else {
        return None;
    };
    let expires = items.first()?.as_f64()?;
    let Value::Binary(ticket) = items.get(1)? else {
        return None;
    };
    Some(Ticket {
        expires,
        ticket: ticket.as_slice().try_into().ok()?,
    })
}

/// Set the ticket in a field map, in place if one is there. A non-map is left alone, for
/// `prepare` to refuse.
pub fn set_ticket_field(fields: &mut Value, ticket: &Ticket) {
    let Value::Map(entries) = fields else {
        return;
    };
    let value = Value::Array(vec![
        Value::F64(ticket.expires),
        Value::Binary(ticket.ticket.to_vec()),
    ]);
    match entries
        .iter_mut()
        .find(|(key, _)| key.as_u64() == Some(u64::from(super::FIELD_TICKET)))
    {
        Some((_, existing)) => *existing = value,
        None => entries.push((Value::from(super::FIELD_TICKET), value)),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TicketBookError {
    #[error("ticket-book snapshot could not be encoded")]
    Encode,
    #[error("ticket-book snapshot has the wrong shape")]
    Invalid,
    #[error("unsupported ticket-book snapshot version {0}")]
    UnsupportedVersion(u64),
}
