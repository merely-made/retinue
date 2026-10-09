use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use std::net::Ipv6Addr;

use super::peers::{Data, Table, Token};
use super::*;

fn ip(text: &str) -> Ipv6Addr {
    text.parse().unwrap()
}

fn netif(name: &str, lls: &[&str]) -> NetIf {
    NetIf {
        name: name.to_string(),
        index: 7,
        up: true,
        multicast: true,
        v4: Vec::new(),
        link_local: lls.iter().map(|a| ip(a)).collect(),
    }
}

#[test]
fn select_follows_stock_skip_lists_and_adopts_the_last_link_local() {
    let ifs = [
        netif("lo0", &["fe80::1"]),
        netif("awdl0", &["fe80::a"]),
        netif("llw0", &["fe80::b"]),
        netif("en5", &["fe80::c"]),
        netif("en1", &["fe80::1:1", "fe80::1:2"]),
        netif("en2", &[]),
        NetIf {
            up: false,
            ..netif("en3", &["fe80::3"])
        },
    ];
    let names = |cfg: &AutoConfig| -> Vec<_> {
        select(&ifs, cfg)
            .into_iter()
            .map(|a| (a.name, a.link_local))
            .collect()
    };
    let darwin = cfg!(any(target_os = "macos", target_os = "ios"));
    let mut expected = vec![("en1".to_string(), ip("fe80::1:2"))];
    if !darwin {
        expected.splice(
            0..0,
            [
                ("awdl0", "fe80::a"),
                ("llw0", "fe80::b"),
                ("en5", "fe80::c"),
            ]
            .map(|(n, a)| (n.to_string(), ip(a))),
        );
    }
    assert_eq!(names(&AutoConfig::default()), expected);
    let all = &select(&ifs, &AutoConfig::default());
    let en1 = all.iter().find(|a| a.name == "en1").unwrap();
    assert_eq!(
        en1.all,
        [ip("fe80::1:1"), ip("fe80::1:2")],
        "every address is an echo"
    );

    let listed = AutoConfig {
        devices: vec!["lo0".into(), "awdl0".into(), "en3".into()],
        ..AutoConfig::default()
    };
    assert_eq!(
        names(&listed),
        [("awdl0".to_string(), ip("fe80::a"))],
        "lo0 never"
    );
    let loose = AutoConfig {
        strict: false,
        ignored_devices: vec!["en1".into()],
        ..listed
    };
    let got = names(&loose);
    assert_eq!(
        got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["awdl0", "en3"]
    );
}

#[test]
fn link_local_is_kept_while_present() {
    let (a, b) = (ip("fe80::a"), ip("fe80::b"));
    assert_eq!(
        keep_link_local(a, &[a, b]),
        Some(a),
        "no flip with two addresses"
    );
    assert_eq!(keep_link_local(a, &[b]), Some(b));
    assert_eq!(keep_link_local(a, &[]), None);
}

const PEER: u32 = 7;

fn table() -> Table<u8> {
    let mut t = Table::default();
    t.adopt(PEER, ip("fe80::1"), &[ip("fe80::1"), ip("fe80::9")], 0);
    t
}

#[test]
fn tokens_echo_new_refresh() {
    let mut t = table();
    assert_eq!(t.on_token(ip("fe80::9"), PEER, 100), Token::Echo);
    assert!(
        !t.carriers()[0].echoed,
        "only the adopted address records the echo"
    );
    assert_eq!(t.on_token(ip("fe80::1"), PEER, 100), Token::Echo);
    assert!(t.carriers()[0].echoed);
    assert_eq!(t.on_token(ip("fe80::2"), PEER, 100), Token::New);
    t.insert(ip("fe80::2"), PEER, 1, 100);
    assert_eq!(t.on_token(ip("fe80::2"), PEER, 900), Token::Refresh);
    assert_eq!(t.peers()[0].last_heard, 900);
    assert_eq!(
        t.on_token(ip("fe80::2"), PEER + 1, 900),
        Token::New,
        "per link"
    );
}

#[test]
fn data_from_peers_only_and_once_across_peers() {
    let mut t = table();
    t.insert(ip("fe80::2"), PEER, 2, 0);
    t.insert(ip("fe80::3"), PEER, 3, 0);
    assert_eq!(t.on_data(ip("fe80::4"), PEER, b"x", 10), Data::Unknown);
    assert_eq!(
        t.on_data(ip("fe80::2"), PEER, b"frame", 10),
        Data::Admit(&2)
    );
    assert_eq!(
        t.on_data(ip("fe80::3"), PEER, b"frame", 759),
        Data::Duplicate
    );
    assert_eq!(t.peers()[1].last_heard, 0, "a duplicate does not refresh");
    assert_eq!(
        t.on_data(ip("fe80::3"), PEER, b"frame", 760),
        Data::Admit(&3)
    );
}

#[test]
fn tick_expires_reverses_and_tracks_the_carrier() {
    let mut t = table();
    t.insert(ip("fe80::2"), PEER, 2, 0);
    t.insert(ip("fe80::3"), PEER, 3, 0);
    let first = t.tick(PEER, 4_000);
    assert!(first.expired.is_empty() && first.reverse.is_empty());
    assert_eq!(first.carrier, None);

    t.on_token(ip("fe80::3"), PEER, 20_000);
    let second = t.tick(PEER, 8_000);
    assert_eq!(
        second.reverse,
        [ip("fe80::2"), ip("fe80::3")],
        "5.2 s since creation"
    );
    assert_eq!(second.carrier, Some(false), "no own token for 6.5 s");

    t.on_token(ip("fe80::1"), PEER, 22_500);
    let third = t.tick(PEER, 22_001);
    assert_eq!(third.expired.len(), 1);
    assert_eq!(third.expired[0].addr, ip("fe80::2"));
    assert_eq!(third.carrier, Some(true));
    assert_eq!(third.reverse, [ip("fe80::3")]);
    assert!(
        t.tick(PEER, 23_000).reverse.is_empty(),
        "at most one per 5.2 s"
    );
    assert_eq!(t.drain(PEER).len(), 1);
    assert!(t.peers().is_empty());
}

#[test]
fn peers_are_capped_per_interface() {
    let mut t = table();
    let addr = |n: usize| Ipv6Addr::from(0xfe80_u128 << 112 | 0x100 | n as u128);
    for n in 0..MAX_PEERS {
        assert_eq!(t.on_token(addr(n), PEER, 0), Token::New);
        t.insert(addr(n), PEER, 0, 0);
    }
    assert_eq!(t.on_token(addr(MAX_PEERS), PEER, 0), Token::Full);
    assert_eq!(
        t.on_token(addr(0), PEER, 5),
        Token::Refresh,
        "known peers stay"
    );
    assert_eq!(
        t.on_token(addr(MAX_PEERS), PEER + 1, 0),
        Token::New,
        "per interface"
    );
    t.tick(PEER, 30_000);
    assert_eq!(
        t.on_token(addr(MAX_PEERS), PEER, 30_000),
        Token::New,
        "room after expiry"
    );
}
