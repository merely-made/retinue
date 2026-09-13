//! Independent Python MessagePack fixtures of stock NomadNet request values.
use retinue::{
    hash::AddressHash,
    request::{Request, StringMapLimits, StringMapRequest},
};
use std::collections::BTreeMap;

fn limits() -> StringMapLimits {
    StringMapLimits::default()
}
fn envelope(value: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x93, 0xcb, 0, 0, 0, 0, 0, 0, 0, 0, 0xc4, 16];
    bytes.extend_from_slice(AddressHash::of(b"/page/capture.mu").as_slice());
    bytes.extend_from_slice(value);
    bytes
}

#[test]
fn stock_nomadnet_values_preserve_types_and_edited_content() {
    let defaults = StringMapRequest::unpack(
        include_bytes!("fixtures/micron_forms/defaults.msgpack"),
        limits(),
    )
    .unwrap();
    assert_eq!(defaults.data.len(), 6);
    assert_eq!(defaults.data["field_mnprobe_checks"], "red,blue");
    assert_eq!(defaults.data["field_mnprobe_empty"], "");
    assert_eq!(defaults.data["field_mnprobe_mask"], "mask-seed");
    let edited = StringMapRequest::unpack(
        include_bytes!("fixtures/micron_forms/edited.msgpack"),
        limits(),
    )
    .unwrap();
    assert_eq!(edited.path_hash, AddressHash::of(b"/page/capture.mu"));
    assert_eq!(edited.data["field_mnprobe_text"], "edited café 雪");
    assert_eq!(
        edited.data["field_mnprobe_multiline"],
        "first line\nsecond 雪"
    );
    let selected = StringMapRequest::unpack(
        include_bytes!("fixtures/micron_forms/selected.msgpack"),
        limits(),
    )
    .unwrap();
    assert_eq!(selected.data.len(), 4);
    assert_eq!(selected.data["field_mnprobe_checks"], "blue");
    assert_eq!(selected.data["field_mnprobe_radio"], "blue");
    assert_eq!(selected.data["var_mnprobe_fixed"], "ready");
    assert!(!selected.data.contains_key("field_mnprobe_mask"));
}

#[test]
fn native_map_is_not_binary_wrapped_and_empty_map_is_not_nil() {
    let request = StringMapRequest::new(
        b"/page/capture.mu",
        BTreeMap::from([("x".into(), "雪".into())]),
        0.0,
    );
    assert_eq!(
        request.pack(limits()).unwrap(),
        envelope(&[0x81, 0xa1, b'x', 0xa3, 0xe9, 0x9b, 0xaa])
    );
    assert!(Request::unpack(&request.pack(limits()).unwrap()).is_err());
    let empty = StringMapRequest::new(b"/page/capture.mu", BTreeMap::new(), 0.0);
    assert_eq!(empty.pack(limits()).unwrap(), envelope(&[0x80]));
    assert!(StringMapRequest::unpack(&envelope(&[0xc0]), limits()).is_err());
    assert!(StringMapRequest::unpack(&envelope(&[0xc4, 1, 0x80]), limits()).is_err());
}

#[test]
fn malformed_or_ambiguous_maps_fail_closed() {
    for value in [
        vec![0x82, 0xa1, b'x', 0xa0, 0xa1, b'x', 0xa0], // duplicate key
        vec![0x81, 0xa1, b'x', 0xc4, 0],                // binary value
        vec![0x81, 0xa1, b'x', 0xa1, 0xff],             // invalid UTF-8
        vec![0x81, 0xa1, b'x', 0xdb, 0xff, 0xff, 0xff, 0xff], // oversized/truncated string
        vec![0xdf, 0xff, 0xff, 0xff, 0xff],             // oversized map count
        vec![0x80, 0xc0],                               // extra value
    ] {
        assert!(StringMapRequest::unpack(&envelope(&value), limits()).is_err());
    }
    let valid = include_bytes!("fixtures/micron_forms/edited.msgpack");
    for end in 0..valid.len() {
        assert!(StringMapRequest::unpack(&valid[..end], limits()).is_err());
    }
}

#[test]
fn budgets_are_checked_on_both_encode_and_decode() {
    let request = StringMapRequest::new(
        b"/page/capture.mu",
        BTreeMap::from([("x".into(), "y".into())]),
        0.0,
    );
    let bytes = request.pack(limits()).unwrap();
    let exact = StringMapLimits {
        max_entries: 1,
        max_encoded_bytes: bytes.len(),
    };
    assert_eq!(request.pack(exact).unwrap(), bytes);
    assert_eq!(StringMapRequest::unpack(&bytes, exact).unwrap(), request);
    for budget in [
        StringMapLimits {
            max_entries: 0,
            ..exact
        },
        StringMapLimits {
            max_encoded_bytes: bytes.len() - 1,
            ..exact
        },
    ] {
        assert!(request.pack(budget).is_err());
        assert!(StringMapRequest::unpack(&bytes, budget).is_err());
    }
}

#[test]
fn accepts_messagepack_string_and_map_widths() {
    // Explicit non-minimal headers independently specified by MessagePack.
    for value in [
        vec![0xde, 0, 1, 0xd9, 1, b'x', 0xda, 0, 1, b'y'],
        vec![
            0xdf, 0, 0, 0, 1, 0xdb, 0, 0, 0, 1, b'x', 0xdb, 0, 0, 0, 1, b'y',
        ],
    ] {
        let map = StringMapRequest::unpack(&envelope(&value), limits()).unwrap();
        assert_eq!(map.data, BTreeMap::from([("x".into(), "y".into())]));
    }
}

#[test]
fn rejects_nonfinite_timestamps() {
    for time in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let request = StringMapRequest::new(b"/page/capture.mu", BTreeMap::new(), time);
        assert!(request.pack(limits()).is_err());
        let mut bytes = envelope(&[0x80]);
        bytes[2..10].copy_from_slice(&time.to_be_bytes());
        assert!(StringMapRequest::unpack(&bytes, limits()).is_err());
    }
}
