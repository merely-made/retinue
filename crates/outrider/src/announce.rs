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
/// Stock LXMF emits no application data when the display name is absent. When present, the
/// wire value is `[display_name, stamp_cost, supported_features]`, with the name as
/// MessagePack binary, the cost as an unsigned integer or nil, and the features as an array
/// of feature numbers. LXMF 0.9.6 emitted only the first two; 1.1.1 appends the third.
///
/// The feature list is a *declaration*, and the one feature defined so far is compression
/// (`SF_COMPRESSION = 0`). Its default is permissive in the dangerous direction: stock reads
/// an absent or nil feature list as compression **supported**. Outrider implements no
/// compression, so emitting the two-element form would silently claim a capability we do not
/// have and invite peers to compress at us. We therefore always emit the three-element form
/// with an empty feature list, which is the only shape that truthfully declares nothing.
///
/// Inbound, we accept two or more elements and ignore anything past the cost. Stock itself
/// parses a four-element announce without complaint, so matching that tolerance costs nothing
/// and is what keeps the next appended field from breaking us the way this one did. The
/// peer's own feature list is not retained because outrider never compresses, so it has no
/// decision to make with it.
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
            &Value::Array(vec![
                Value::Binary(display_name.clone()),
                cost,
                Value::Array(Vec::new()),
            ]),
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
        if parts.len() < 2 {
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
    #[error("LXMF delivery announce must be an array of at least two items")]
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
///
/// The accepted identity is deliberately NOT written into the address book: an IDENTIFY is
/// not an announce — it carries no app_data, no stamp cost, and no claim of reachability —
/// so it authenticates this session and nothing beyond it. The same sender on a new link
/// without an IDENTIFY starts over.
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
    fn stock_0_9_6_two_element_announce_still_decodes() {
        let captured = hex::decode("92c40c53746f636b204f7261636c6508").unwrap();
        let announce = DeliveryAnnounce::decode(&captured).unwrap();
        assert_eq!(
            announce.display_name.as_deref(),
            Some(b"Stock Oracle".as_slice())
        );
        assert_eq!(announce.stamp_cost, Some(8));
    }

    #[test]
    fn stock_1_1_1_three_element_announce_decodes() {
        // Captured from stock LXMF 1.1.1: [b"Stock Receiver", 8, [0]]. The trailing [0]
        // declares compression support; we read the name and cost and ignore it.
        let captured = hex::decode("93c40e53746f636b205265636569766572089100").unwrap();
        let announce = DeliveryAnnounce::decode(&captured).unwrap();
        assert_eq!(
            announce.display_name.as_deref(),
            Some(b"Stock Receiver".as_slice())
        );
        assert_eq!(announce.stamp_cost, Some(8));
    }

    #[test]
    fn stock_1_1_1_nil_stamp_cost_announce_decodes() {
        // [b"Stock Opportunistic Receiver", nil, [0]], captured from stock LXMF 1.1.1.
        let captured =
            hex::decode("93c41c53746f636b204f70706f7274756e6973746963205265636569766572c09100")
                .unwrap();
        let announce = DeliveryAnnounce::decode(&captured).unwrap();
        assert_eq!(announce.stamp_cost, None);
    }

    /// The bug this guards against is the one that broke us: LXMF 1.1.1 appended a third
    /// element and a `!= 2` length check refused every stock announce, so outrider never
    /// learned any sender's keys. Stock parses a four-element announce happily; so do we.
    #[test]
    fn announces_longer_than_we_understand_are_accepted() {
        let future = hex::decode("94c4014e089100 63".replace(' ', "").as_str()).unwrap();
        let announce = DeliveryAnnounce::decode(&future).unwrap();
        assert_eq!(announce.display_name.as_deref(), Some(b"N".as_slice()));
        assert_eq!(announce.stamp_cost, Some(8));
    }

    /// Outrider implements no compression. An empty feature list is the only encoding that
    /// says so: stock reads both an absent list and a nil list as compression *supported*.
    #[test]
    fn we_declare_an_empty_feature_list() {
        let encoded = DeliveryAnnounce {
            display_name: Some(b"Stock Oracle".to_vec()),
            stamp_cost: Some(8),
        }
        .encode()
        .unwrap();
        assert_eq!(hex::encode(&encoded), "93c40c53746f636b204f7261636c650890");

        let anonymous_cost = DeliveryAnnounce::named("Stock Receiver").encode().unwrap();
        assert_eq!(
            hex::encode(&anonymous_cost),
            "93c40e53746f636b205265636569766572c090"
        );
    }

    /// What we emit must survive our own decoder, feature list and all.
    #[test]
    fn our_own_announce_round_trips() {
        let announce = DeliveryAnnounce {
            display_name: Some(b"outrider".to_vec()),
            stamp_cost: Some(4),
        };
        let encoded = announce.encode().unwrap();
        assert_eq!(DeliveryAnnounce::decode(&encoded).unwrap(), announce);
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
