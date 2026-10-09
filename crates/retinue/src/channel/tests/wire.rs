//! Gold tests against the RNS 1.3.8 wire captures.

use super::{hex_bytes, hex_str};
use crate::channel::{Envelope, MAX_DATA_LEN, STREAM_ID_MAX, STREAM_MSGTYPE, StreamFrame};

#[test]
fn envelope_matches_rns_capture() {
    // Gold test: the encoding equals RNS 1.3.8's Envelope.pack() for every captured vector.
    let fixture = include_str!("../../../tests/fixtures/channel_wire.json");
    let doc: serde_json::Value = serde_json::from_str(fixture).unwrap();
    for v in doc["envelope_vectors"].as_array().unwrap() {
        let msgtype = v["msgtype"].as_u64().unwrap() as u16;
        let sequence = v["sequence"].as_u64().unwrap() as u16;
        let payload = hex_bytes(v["payload_hex"].as_str().unwrap());
        let expected = v["packed_hex"].as_str().unwrap();
        let env = Envelope {
            msgtype,
            sequence,
            payload,
        };
        assert_eq!(
            hex_str(&env.encode()),
            expected,
            "encode must equal RNS pack()"
        );
        assert_eq!(Envelope::decode(&env.encode()), Some(env), "round-trip");
    }
}

#[test]
fn stream_frame_matches_rns_capture() {
    // Gold test: retinue's StreamFrame encoding equals RNS 1.3.8's own
    // StreamDataMessage.pack() for every captured vector, and our constants match.
    let fixture = include_str!("../../../tests/fixtures/buffer_wire.json");
    let doc: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let c = &doc["constants"];
    assert_eq!(c["MSGTYPE"].as_u64().unwrap() as u16, STREAM_MSGTYPE);
    assert_eq!(c["STREAM_ID_MAX"].as_u64().unwrap() as u16, STREAM_ID_MAX);
    assert_eq!(c["MAX_DATA_LEN"].as_u64().unwrap() as usize, MAX_DATA_LEN);
    for v in doc["frame_vectors"].as_array().unwrap() {
        let frame = StreamFrame {
            stream_id: v["stream_id"].as_u64().unwrap() as u16,
            eof: v["eof"].as_bool().unwrap(),
            compressed: v["compressed"].as_bool().unwrap(),
            data: hex_bytes(v["data_hex"].as_str().unwrap()),
        };
        let expected = v["packed_hex"].as_str().unwrap();
        assert_eq!(
            hex_str(&frame.encode()),
            expected,
            "encode must equal RNS pack()"
        );
        assert_eq!(
            StreamFrame::decode(&frame.encode()),
            Some(frame),
            "round-trip"
        );
    }
}
