use super::*;
use std::string::ToString;

#[test]
fn default_group_matches_stock() {
    let g = discovery_group(DEFAULT_GROUP, Scope::Link, AddrType::Temporary);
    let expected: Ipv6Addr = "ff12:0:d70b:fb1c:16e4:5e39:485e:31e1".parse().unwrap();
    assert_eq!(g, expected);
}

#[test]
fn descope_clears_kame_scope_only_on_link_local() {
    let kame: Ipv6Addr = "fe80:4::aede:48ff:fe00:1122".parse().unwrap();
    assert_eq!(
        descope(kame),
        "fe80::aede:48ff:fe00:1122".parse::<Ipv6Addr>().unwrap()
    );
    let long_run: Ipv6Addr = "fe80:4:0:0:1::".parse().unwrap();
    assert_eq!(descope(long_run).to_string(), "fe80::1:0:0:0");
    let ula: Ipv6Addr = "fd00:4::1".parse().unwrap();
    assert_eq!(descope(ula), ula);
}

#[test]
fn token_covers_group_and_text() {
    let ll: Ipv6Addr = "fe80::1".parse().unwrap();
    let mut data = [0u8; 40];
    data[..32].copy_from_slice(&peering_token(DEFAULT_GROUP, &ll));
    assert!(token_valid(DEFAULT_GROUP, &ll, &data));
    assert!(!token_valid(b"other", &ll, &data));
    assert!(!token_valid(
        DEFAULT_GROUP,
        &"fe80::2".parse().unwrap(),
        &data
    ));
    assert!(!token_valid(DEFAULT_GROUP, &ll, &data[..31]));
}

#[test]
fn config_names() {
    assert_eq!(
        Scope::from_config("Organisation"),
        Some(Scope::Organisation)
    );
    assert_eq!(Scope::from_config("planet"), None);
    assert_eq!(AddrType::from_config("PERMANENT"), AddrType::Permanent);
    assert_eq!(AddrType::from_config("bogus"), AddrType::Temporary);
}

/// `tests/fixtures/auto_vectors.json`, from stock 1.5.7 by `oracle/interop_auto_vectors.py`.
#[test]
fn stock_vectors() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/auto_vectors.json")).unwrap();
    let s = |v: &serde_json::Value| v.as_str().unwrap().to_string();
    let addr = |v: &serde_json::Value| s(v).parse::<Ipv6Addr>().unwrap();
    for g in v["groups"].as_array().unwrap() {
        let scope = Scope::from_config(&s(&g["scope"])).unwrap();
        let kind = AddrType::from_config(&s(&g["type"]));
        let got = discovery_group(s(&g["group"]).as_bytes(), scope, kind);
        assert_eq!(got, addr(&g["address"]), "{g}");
    }
    for d in v["descope"].as_array().unwrap() {
        let input = s(&d["input"]);
        let got = descope(input.split('%').next().unwrap().parse().unwrap());
        // Stock's regex misses the long zero run; peers hash the cleared form.
        let expected = match input.as_str() {
            "fe80:4:0:0:1::" => "fe80::1:0:0:0".to_string(),
            _ => s(&d["stock"]),
        };
        assert_eq!(got.to_string(), expected);
    }
    let tokens = v["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 72);
    for t in tokens {
        let ll = addr(&t["text"]);
        assert_eq!(
            ll.to_string(),
            s(&t["text"]),
            "RFC 5952 text matches Python's"
        );
        let token = peering_token(s(&t["group"]).as_bytes(), &ll);
        assert_eq!(hex::encode(token), s(&t["token"]));
    }
}

#[test]
fn ring_expires_and_evicts() {
    let mut ring = HashRing::<2>::default();
    ring.insert([1; 32], 1_000);
    assert!(ring.fresh(&[1; 32], 1_749, 750));
    assert!(!ring.fresh(&[1; 32], 1_750, 750));
    ring.insert([2; 32], 1_100);
    ring.insert([3; 32], 1_200);
    assert!(
        !ring.fresh(&[1; 32], 1_300, 750),
        "the oldest entry was evicted"
    );
    assert!(ring.fresh(&[2; 32], 1_300, 750) && ring.fresh(&[3; 32], 1_300, 750));
}
