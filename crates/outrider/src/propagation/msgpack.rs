//! MessagePack framing for announces, fetch requests and responses.

use std::io::Cursor;

use rmpv::Value;

use super::{
    DEFAULT_MAX_PROPAGATION_ENTRIES, FETCH_PATH_HASH, PropagationError, PropagationMessage,
};

type FetchSelection = (Vec<[u8; 32]>, Vec<[u8; 32]>, u64);

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

pub(super) fn decode_offer_request(bytes: &[u8]) -> Result<(), PropagationError> {
    let data = decode_fetch_request(bytes)?;
    match data {
        Value::Array(parts) if parts == vec![Value::Nil, Value::Nil] => Ok(()),
        _ => Err(PropagationError::InvalidFetchRequest),
    }
}

pub(super) fn decode_fetch_selection(bytes: &[u8]) -> Result<FetchSelection, PropagationError> {
    let Value::Array(parts) = decode_fetch_request(bytes)? else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    if parts.len() != 3 {
        return Err(PropagationError::InvalidFetchRequest);
    }
    let Value::Array(wanted) = &parts[0] else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    let Value::Array(handled) = &parts[1] else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    let limit = parts[2]
        .as_u64()
        .ok_or(PropagationError::InvalidFetchRequest)?;
    Ok((decode_ids(wanted)?, decode_ids(handled)?, limit))
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

pub(super) fn decode_ids(values: &[Value]) -> Result<Vec<[u8; 32]>, PropagationError> {
    values
        .iter()
        .map(|value| match value {
            Value::Binary(id) if id.len() == 32 => Ok(id
                .as_slice()
                .try_into()
                .expect("checked transient id length")),
            _ => Err(PropagationError::InvalidFetchRequest),
        })
        .collect()
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
