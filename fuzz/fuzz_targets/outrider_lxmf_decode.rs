#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use outrider::portable::{Payload, decode, encode_payload};

/// A real LXMF object, so mutation starts from something that parses.
const FIXTURE: &[u8] = include_bytes!("../seeds/outrider-lxmf-decode/stock-0-9-6.seed");

fn mutate(message: &mut [u8], bytes: &[u8]) {
    if message.is_empty() {
        return;
    }
    for (index, byte) in bytes.iter().take(12).enumerate() {
        let offset = (usize::from(*byte) + index * 31) % message.len();
        message[offset] ^= byte.rotate_left((index % 8) as u32);
    }
}

fuzz_target!(|input: &[u8]| {
    // The board's LXMF ingress. This is the one parser in the family that meets bytes chosen
    // by a stranger on hardware whose entire stack is measured in kilobytes, so what is being
    // asserted here is not correctness of the result but that no input reaches a panic, an
    // unbounded recursion, or an allocation proportional to a number the sender chose.
    //
    // Every branch below ends in a `Result`. A crash is the finding.
    let Some((&selector, body)) = input.split_first() else {
        return;
    };

    let message = match selector & 0b11 {
        // Raw bytes: the header-length and shape checks, mostly.
        0 => body.to_vec(),
        // A stock message, unmodified: the path that must keep working.
        1 => FIXTURE.to_vec(),
        // A stock message with the fuzzer's damage applied, so mutations keep enough
        // structure to reach past the header into the payload's own parsing.
        2 => {
            let mut wire = FIXTURE.to_vec();
            mutate(&mut wire, body);
            wire
        }
        // A payload this codec built itself, then damaged. This reaches the field-map
        // skipper with a map that was well-formed a moment ago, which is where a
        // length or nesting bound is most likely to be wrong by one.
        _ => {
            let mut payload = Payload::text(1_753_603_200.5, b"t", body);
            // The fields map is carried verbatim and never interpreted, so letting the
            // fuzzer choose it is the point: this is the byte range a hostile sender
            // controls most directly.
            if !body.is_empty() {
                payload.fields = body.to_vec();
            }
            let Ok(encoded) = encode_payload(&payload, false) else {
                return;
            };
            let mut wire = vec![0_u8; 96];
            wire.extend_from_slice(&encoded);
            mutate(&mut wire, body);
            wire
        }
    };

    if let Ok(decoded) = decode(&message) {
        // Round-tripping a decoded message must not panic either, and it is where a
        // length recorded during decode could disagree with one computed during encode.
        let _ = encode_payload(&decoded.payload, decoded.payload.stamp.is_some());
        let _ = decoded.signing_bytes();
    }
});
