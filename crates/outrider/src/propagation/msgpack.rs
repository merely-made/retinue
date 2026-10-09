//! MessagePack framing for announces, fetch requests and responses.

use std::io::Cursor;

use rmpv::Value;

use super::{DEFAULT_MAX_PROPAGATION_ENTRIES, FETCH_PATH_HASH, PropagationError};

type FetchSelection = (Vec<[u8; 32]>, Vec<[u8; 32]>, u64);

const ERROR_NO_IDENTITY: u64 = 0xf0;
const ERROR_NO_ACCESS: u64 = 0xf1;

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
    // A node answers with a bare error code instead of a list (`LXMPeer.py` ERROR_NO_*).
    match envelope.pop().expect("two-item response") {
        Value::Integer(code) if code.as_u64() == Some(ERROR_NO_IDENTITY) => {
            Err(PropagationError::NoIdentity)
        }
        Value::Integer(code) if code.as_u64() == Some(ERROR_NO_ACCESS) => {
            Err(PropagationError::NoAccess)
        }
        value => Ok(value),
    }
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

/// The raw entries of a `/get` response, each decoded by the caller so one bad entry does
/// not cost the others.
pub(super) fn decode_entry_response(bytes: &[u8]) -> Result<Vec<Vec<u8>>, PropagationError> {
    let Value::Array(values) = decode_response(bytes)? else {
        return Err(PropagationError::InvalidFetchResponse);
    };
    if values.len() > DEFAULT_MAX_PROPAGATION_ENTRIES {
        return Err(PropagationError::TooManyEntries);
    }
    values
        .into_iter()
        .map(|value| match value {
            Value::Binary(entry) => Ok(entry),
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
