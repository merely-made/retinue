//! MessagePack framing for announces, fetch requests and responses.

use std::io::Cursor;

use rmpv::Value;

use super::{
    DEFAULT_MAX_PROPAGATION_ENTRIES, FETCH_PATH_HASH, PropagationError, PropagationMessage,
};

pub(super) fn encode_value(value: &Value) -> Result<Vec<u8>, PropagationError> {
    let mut encoded = Vec::new();
    rmpv::encode::write_value(&mut encoded, value).map_err(|_| PropagationError::Encode)?;
    Ok(encoded)
}

pub(super) fn encode_fetch_request(time: f64, data: Value) -> Result<Vec<u8>, PropagationError> {
    encode_value(&Value::Array(vec![
        Value::F64(time),
        Value::Binary(FETCH_PATH_HASH.to_vec()),
        data,
    ]))
}

pub(super) fn decode_response(bytes: &[u8]) -> Result<Value, PropagationError> {
    let Value::Array(mut envelope) = decode_one(bytes)? else {
        return Err(PropagationError::InvalidFetchResponse);
    };
    if envelope.len() != 2
        || !matches!(&envelope[0], Value::Binary(request_id) if request_id.len() == 16)
    {
        return Err(PropagationError::InvalidFetchResponse);
    }
    Ok(envelope.pop().expect("two-item response"))
}

pub(super) fn decode_id_response(bytes: &[u8]) -> Result<Vec<[u8; 32]>, PropagationError> {
    let Value::Array(values) = decode_response(bytes)? else {
        return Err(PropagationError::InvalidFetchResponse);
    };
    values
        .into_iter()
        .map(|value| match value {
            Value::Binary(id) if id.len() == 32 => {
                Ok(id.try_into().expect("checked transient id length"))
            }
            _ => Err(PropagationError::InvalidFetchResponse),
        })
        .collect()
}

pub(super) fn decode_entry_response(
    bytes: &[u8],
    max_entry_bytes: usize,
) -> Result<Vec<PropagationMessage>, PropagationError> {
    let Value::Array(values) = decode_response(bytes)? else {
        return Err(PropagationError::InvalidFetchResponse);
    };
    if values.len() > DEFAULT_MAX_PROPAGATION_ENTRIES {
        return Err(PropagationError::TooManyEntries);
    }
    values
        .into_iter()
        .map(|value| match value {
            Value::Binary(entry) => PropagationMessage::decode(&entry, max_entry_bytes),
            _ => Err(PropagationError::InvalidFetchResponse),
        })
        .collect()
}

/// A `/get` request: an offer of what is held, or a fetch that may also acknowledge.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum GetRequest {
    Offer,
    Fetch {
        wanted: Vec<[u8; 32]>,
        handled: Vec<[u8; 32]>,
        /// The client's response budget in KB, integer or float (`LXMRouter.py` 1532-1537).
        limit_kb: Option<f64>,
    },
}

/// Read `[nil|ids, nil|ids, limit?]`. Both nil asks for an offer; anything that is not
/// a 32-byte id is skipped, as stock's membership test skips it (`LXMRouter.py` 1499-1560).
pub(super) fn decode_get_request(data: &Value) -> Result<GetRequest, PropagationError> {
    let Value::Array(parts) = data else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    if parts.len() < 2 {
        return Err(PropagationError::InvalidFetchRequest);
    }
    let ids = |value: &Value| match value {
        Value::Nil => Ok(None),
        Value::Array(ids) => Ok(Some(
            ids.iter()
                .filter_map(|id| id.as_slice()?.try_into().ok())
                .collect::<Vec<[u8; 32]>>(),
        )),
        _ => Err(PropagationError::InvalidFetchRequest),
    };
    Ok(match (ids(&parts[0])?, ids(&parts[1])?) {
        (None, None) => GetRequest::Offer,
        (wanted, handled) => GetRequest::Fetch {
            wanted: wanted.unwrap_or_default(),
            handled: handled.unwrap_or_default(),
            limit_kb: parts.get(2).and_then(Value::as_f64),
        },
    })
}

pub(super) fn decode_fetch_request(bytes: &[u8]) -> Result<Value, PropagationError> {
    let Value::Array(mut parts) = decode_one(bytes)? else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    if parts.len() != 3
        || !matches!(&parts[0], Value::F64(time) if time.is_finite())
        || !matches!(&parts[1], Value::Binary(path) if path.as_slice() == FETCH_PATH_HASH)
    {
        return Err(PropagationError::InvalidFetchRequest);
    }
    Ok(parts.pop().expect("three-item request"))
}

pub(super) fn decode_one(bytes: &[u8]) -> Result<Value, PropagationError> {
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value(&mut cursor)
        .map_err(|_| PropagationError::MalformedMessagePack)?;
    if cursor.position() as usize != bytes.len() {
        return Err(PropagationError::MalformedMessagePack);
    }
    Ok(value)
}

pub(super) fn byte(value: &Value) -> Result<u8, PropagationError> {
    value
        .as_u64()
        .and_then(|value| value.try_into().ok())
        .ok_or(PropagationError::InvalidAnnounce)
}
