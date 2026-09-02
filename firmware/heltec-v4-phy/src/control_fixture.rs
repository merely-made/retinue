//! Target-side WN0 codec check.
//!
//! This is deliberately a boot check rather than a host test. The firmware target compiles
//! and executes the same decode/encode path that receives management frames, using only fixed
//! stack buffers and the shared vectors owned by `radio-hand`.

use radio_hand::control::{
    GOLDEN_REQUEST, GOLDEN_RESPONSE, MAX_REQUEST_LEN, MAX_RESPONSE_LEN, decode_request,
    decode_response, encode_request, encode_response,
};

/// Verify that this firmware target consumes and reproduces both WN0 golden frames exactly.
pub(crate) fn verify() {
    let request = decode_request(&GOLDEN_REQUEST).unwrap_or_else(|_| panic!());
    let mut request_out = [0_u8; MAX_REQUEST_LEN];
    let request_len = encode_request(&request, &mut request_out).unwrap_or_else(|_| panic!());
    if request_len != GOLDEN_REQUEST.len() || request_out[..request_len] != GOLDEN_REQUEST {
        panic!();
    }

    let response = decode_response(&GOLDEN_RESPONSE).unwrap_or_else(|_| panic!());
    let mut response_out = [0_u8; MAX_RESPONSE_LEN];
    let response_len = encode_response(&response, &mut response_out).unwrap_or_else(|_| panic!());
    if response_len != GOLDEN_RESPONSE.len() || response_out[..response_len] != GOLDEN_RESPONSE {
        panic!();
    }
}
