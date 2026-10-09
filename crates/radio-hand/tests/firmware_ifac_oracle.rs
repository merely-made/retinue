#![cfg(any(feature = "instances", feature = "replay"))]

use radio_hand::retinue_carrier::RetinueCarrier;
use retinue::packet::{HeaderType, Propagation};
use retinue::{Error, Ifac, Packet};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../retinue/tests/fixtures/rns_ifac_1_5_4.json"
    ))
    .unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap().as_bytes();
    text.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn protected(data: &Value) -> RetinueCarrier {
    RetinueCarrier::protected(
        Ifac::for_serial(data["network_name"].as_str(), data["passphrase"].as_str()).unwrap(),
    )
}

#[test]
fn actual_stock_rns_1_5_4_frames_match_and_obey_the_radio_boundary() {
    let data = fixture();
    assert_eq!(data["producer"]["rns_version"], "1.5.4");
    assert_eq!(data["producer"]["distribution_version"], "1.5.4");
    let carrier = protected(&data);
    assert_eq!(carrier.logical_mtu(), 247);
    for case in data["cases"].as_array().unwrap() {
        let wire = bytes(&case["wire_hex"]);
        let logical = bytes(&case["logical_hex"]);
        assert_eq!(wire.len(), logical.len() + 8);
        let packet = Packet::decode(&logical).unwrap();
        if case["fits_physical_255"].as_bool().unwrap() {
            assert_eq!(carrier.decode(&wire).unwrap(), packet);
            assert_eq!(carrier.encode(&packet).unwrap(), wire);
        } else {
            assert_eq!(wire.len(), 256);
            assert_eq!(carrier.decode(&wire), Err(Error::Oversize));
            assert_eq!(carrier.encode(&packet), Err(Error::Oversize));
        }
    }
}

#[test]
fn protected_ingress_refuses_wrong_credentials_plain_and_tampered_stock_frames() {
    let data = fixture();
    let carrier = protected(&data);
    let wrong = RetinueCarrier::protected(
        Ifac::for_serial(data["network_name"].as_str(), Some("wrong credential")).unwrap(),
    );
    for case in data["cases"].as_array().unwrap() {
        if !case["fits_physical_255"].as_bool().unwrap() {
            continue;
        }
        let wire = bytes(&case["wire_hex"]);
        let logical = bytes(&case["logical_hex"]);
        assert_eq!(wrong.decode(&wire), Err(Error::BadIfac));
        assert_eq!(carrier.decode(&logical), Err(Error::BadIfac));
        // Every byte is independently changed, including header, access code and payload.
        for offset in 0..wire.len() {
            let mut changed = wire.clone();
            changed[offset] ^= 1;
            assert_eq!(
                carrier.decode(&changed),
                Err(Error::BadIfac),
                "{} byte {offset}",
                case["name"]
            );
        }
    }
}

#[test]
fn local_type2_growth_reserves_sixteen_bytes_before_final_carrier_admission() {
    let data = fixture();
    let carrier = protected(&data);
    // These Type2 packets are local transformations, not stock Type2 captures.
    for case in &data["cases"].as_array().unwrap()[..2] {
        let logical = bytes(&case["logical_hex"]);
        let mut relayed = Packet::decode(&logical).unwrap();
        relayed.header_type = HeaderType::Type2;
        relayed.propagation = Propagation::Transport;
        relayed.transport = Some(retinue::hash::AddressHash::from_bytes([0x42; 16]));
        assert_eq!(relayed.encode().len(), logical.len() + 16);
        if logical.len() == 231 {
            let wire = carrier.encode(&relayed).unwrap();
            assert_eq!(wire.len(), 255);
            assert_eq!(carrier.decode(&wire).unwrap(), relayed);
        } else {
            assert_eq!(logical.len(), 232);
            assert_eq!(carrier.encode(&relayed), Err(Error::Oversize));
        }
    }
}

#[test]
fn plain_carrier_keeps_its_full_physical_budget() {
    let data = fixture();
    let carrier = RetinueCarrier::default();
    assert_eq!(carrier.logical_mtu(), 255);
    let mut packet = Packet::decode(&bytes(&data["cases"][0]["logical_hex"])).unwrap();
    packet.payload.resize(255 - 19, 0x5a);
    let wire = carrier.encode(&packet).unwrap();
    assert_eq!(wire.len(), 255);
    assert_eq!(carrier.decode(&wire).unwrap(), packet);
    packet.payload.push(0x5a);
    assert_eq!(carrier.encode(&packet), Err(Error::Oversize));
    assert_eq!(carrier.decode(&packet.encode()), Err(Error::Oversize));
}
