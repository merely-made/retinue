//! LXMF delivery destination and announce conventions.

use std::io::Cursor;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::hash::AddressHash;
use retinue::identity::Identity;
use rmpv::Value;

pub const DEFAULT_MAX_ANNOUNCE_BYTES: usize = 1024;

/// The application data carried by an `lxmf.delivery` announce.
///
/// Stock LXMF 0.9.6 emits no application data when the display name is absent.
/// When present, the wire value is `[display_name, stamp_cost]`, with the name
/// as MessagePack binary and the cost as an unsigned integer or nil.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeliveryAnnounce {
    pub display_name: Option<Vec<u8>>,
    pub stamp_cost: Option<u8>,
}

impl DeliveryAnnounce {
    pub fn named(display_name: impl Into<Vec<u8>>) -> Self {
        Self {
            display_name: Some(display_name.into()),
            stamp_cost: None,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, AnnounceError> {
        let Some(display_name) = &self.display_name else {
            return Ok(Vec::new());
        };
        let cost = self
            .stamp_cost
            .map_or(Value::Nil, |cost| Value::from(u64::from(cost)));
        let mut encoded = Vec::new();
        rmpv::encode::write_value(
            &mut encoded,
            &Value::Array(vec![Value::Binary(display_name.clone()), cost]),
        )
        .map_err(|_| AnnounceError::Encode)?;
        if encoded.len() > DEFAULT_MAX_ANNOUNCE_BYTES {
            return Err(AnnounceError::TooLarge);
        }
        Ok(encoded)
    }

    pub fn decode(encoded: &[u8]) -> Result<Self, AnnounceError> {
        Self::decode_bounded(encoded, DEFAULT_MAX_ANNOUNCE_BYTES)
    }

    pub fn decode_bounded(encoded: &[u8], max_bytes: usize) -> Result<Self, AnnounceError> {
        if encoded.len() > max_bytes {
            return Err(AnnounceError::TooLarge);
        }
        if encoded.is_empty() {
            return Ok(Self::default());
        }
        let mut cursor = Cursor::new(encoded);
        let value = rmpv::decode::read_value(&mut cursor)
            .map_err(|_| AnnounceError::MalformedMessagePack)?;
        if cursor.position() as usize != encoded.len() {
            return Err(AnnounceError::MalformedMessagePack);
        }
        let Value::Array(parts) = value else {
            return Err(AnnounceError::InvalidShape);
        };
        if parts.len() != 2 {
            return Err(AnnounceError::InvalidShape);
        }
        let Value::Binary(display_name) = &parts[0] else {
            return Err(AnnounceError::InvalidDisplayName);
        };
        let stamp_cost = match &parts[1] {
            Value::Nil => None,
            Value::Integer(value) => value
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or(AnnounceError::InvalidStampCost)
                .map(Some)?,
            _ => return Err(AnnounceError::InvalidStampCost),
        };
        Ok(Self {
            display_name: Some(display_name.clone()),
            stamp_cost,
        })
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AnnounceError {
    #[error("LXMF delivery announce exceeds the configured byte limit")]
    TooLarge,
    #[error("LXMF delivery announce is not one complete MessagePack value")]
    MalformedMessagePack,
    #[error("LXMF delivery announce must be a two-item array")]
    InvalidShape,
    #[error("LXMF delivery display name must be MessagePack binary")]
    InvalidDisplayName,
    #[error("LXMF delivery stamp cost must be nil or an unsigned byte")]
    InvalidStampCost,
    #[error("LXMF delivery announce could not be encoded")]
    Encode,
}

pub fn delivery_name() -> DestinationName {
    DestinationName::new("lxmf", ["delivery"])
}

pub fn delivery_destination(identity: &Identity) -> AddressHash {
    delivery_name().destination_hash(identity)
}

/// Resolve a message source's identity, asking the network for it when we do not have it.
///
/// A message is only verifiable if we hold the sender's keys, and we hold them only from an
/// announce. Nothing obliges a sender to announce before it sends, so a first message from a
/// stranger is unverifiable through no fault of theirs.
///
/// Refusing it is still right: an unverified message must not be delivered. Refusing
/// *silently* is what was wrong, because it made the refusal permanent — every retry hit the
/// same wall, and from the sender's side the recipient simply never answered. Found against
/// MeshChatX 2.0.1 driving a board on the RNode channel: it sent three times, and all three
/// arrived intact and were dropped.
///
/// So a refusal now asks. A path request for the source's delivery destination is answered
/// with an announce, an announce carries the identity, and the sender's next retry verifies.
/// The request is rate-limited per destination inside `retinue`, so a peer sending traffic we
/// cannot verify cannot make us broadcast once per packet.
pub fn resolve_source(endpoint: &Endpoint, source: AddressHash) -> Option<Identity> {
    resolve_source_with_link(endpoint, source, None)
}

/// Resolve a message source, accepting an identity the sender proved on the link it opened.
///
/// `identified` is what the peer signed as part of link setup, so it is *stronger* evidence
/// than an announce: an announce says some destination exists somewhere, while this is the
/// peer on the other end of this link saying who it is. It is still only accepted when it
/// derives to the exact delivery destination the message names as its source, because
/// IDENTIFY proves who the peer is and says nothing about who the payload claims to be from.
///
/// This is what closes the case the path request could not. A path request only helps if the
/// sender answers it, and a client with transport disabled may simply not.
pub fn resolve_source_with_link(
    endpoint: &Endpoint,
    source: AddressHash,
    identified: Option<Identity>,
) -> Option<Identity> {
    if let Some(identity) = endpoint.resolve(source) {
        return Some(identity);
    }
    if let Some(identity) = identified
        && delivery_destination(&identity) == source
    {
        return Some(identity);
    }
    endpoint.request_path(source);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_named_announce_with_stamp_cost_round_trips_exactly() {
        let captured = hex::decode("92c40c53746f636b204f7261636c6508").unwrap();
        let announce = DeliveryAnnounce::decode(&captured).unwrap();
        assert_eq!(
            announce.display_name.as_deref(),
            Some(b"Stock Oracle".as_slice())
        );
        assert_eq!(announce.stamp_cost, Some(8));
        assert_eq!(announce.encode().unwrap(), captured);
    }

    #[test]
    fn stock_named_announce_without_stamp_cost_round_trips_exactly() {
        let captured = hex::decode("92c40e53746f636b205265636569766572c0").unwrap();
        let announce = DeliveryAnnounce::decode(&captured).unwrap();
        assert_eq!(
            announce.display_name.as_deref(),
            Some(b"Stock Receiver".as_slice())
        );
        assert_eq!(announce.stamp_cost, None);
        assert_eq!(announce.encode().unwrap(), captured);
    }

    #[test]
    fn absent_application_data_is_an_anonymous_announce() {
        assert_eq!(
            DeliveryAnnounce::decode(&[]).unwrap(),
            DeliveryAnnounce::default()
        );
        assert_eq!(
            DeliveryAnnounce::default().encode().unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn malformed_or_oversized_announces_are_rejected() {
        assert_eq!(
            DeliveryAnnounce::decode(&[0x91, 0xc0]),
            Err(AnnounceError::InvalidShape)
        );
        assert_eq!(
            DeliveryAnnounce::decode_bounded(&[0x92, 0xc4, 0x00, 0xc0], 3),
            Err(AnnounceError::TooLarge)
        );
    }
}
