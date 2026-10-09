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

/// End to end on lo0, which Darwin gives both `::1` and `fe80::1`; Linux's lo has no
/// link-local.
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod loopback {
    use std::sync::Arc;

    use super::*;
    use crate::destination::DestinationName;
    use crate::endpoint::Endpoint;
    use crate::identity::PrivateIdentity;

    fn free_v6_port() -> u16 {
        std::net::UdpSocket::bind("[::1]:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Two endpoints on lo0, one adopting `::1` and one `fe80::1`. Darwin drops lo0 multicast
    /// from `fe80::1`, so only one side's tokens multicast: the other must learn it by the
    /// reverse unicast token, and data then flows both ways.
    #[tokio::test]
    async fn peers_and_carries_on_loopback() {
        let index = crate::iface::netinfo::find("lo0")
            .unwrap()
            .expect("lo0")
            .index;
        let (discovery, data) = (free_v6_port(), free_v6_port());
        let cfg = AutoConfig {
            group_id: b"retinue-unit".to_vec(),
            discovery_port: discovery,
            data_port: data,
            ifac: Some(Ifac::for_stream(Some("auto-unit"), None).unwrap()),
            ..AutoConfig::default()
        };
        let adopt = |addr: &str| Adopted {
            name: "lo0".into(),
            index,
            link_local: ip(addr),
            all: vec![ip(addr)],
        };
        let ep = |seed| {
            Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
                &[seed; 64],
            )))
        };
        let (a, b) = (ep(0xa1), ep(0xb1));
        let ha = run::attach_adopted(&a, cfg.clone(), vec![adopt("::1")], false).unwrap();
        let hb = run::attach_adopted(&b, cfg, vec![adopt("fe80::1")], false).unwrap();

        let peered =
            |h: &AutoHandle, addr: &str| h.status().peers.iter().any(|p| p.addr == ip(addr));
        for _ in 0..50 {
            if peered(&ha, "fe80::1") && peered(&hb, "::1") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(
            peered(&hb, "::1"),
            "b heard a's multicast: {:?}",
            hb.status()
        );
        assert!(
            peered(&ha, "fe80::1"),
            "a learned b by unicast: {:?}",
            ha.status()
        );
        assert!(
            ha.status().adopted[0].echoed,
            "a's multicast came back to it"
        );

        for (from, to, seed) in [(&a, &b, 1u8), (&b, &a, 2)] {
            let name = DestinationName::new("retinue", ["auto-unit"]);
            from.register(name.clone(), &[seed]);
            from.announce(&name, &[seed]);
            let fact =
                tokio::time::timeout(std::time::Duration::from_secs(5), to.next_announcement())
                    .await
                    .expect("announce crosses the peer interface")
                    .unwrap();
            assert_eq!(fact.app_data, [seed]);
        }
        assert!(ha.status().counters.rx > 0 && hb.status().counters.rx > 0);

        let peer_iface = hb.status().peers[0].interface;
        drop(hb);
        for _ in 0..50 {
            if !b.interface_ids().contains(&peer_iface) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            !b.interface_ids().contains(&peer_iface),
            "closing detaches peers"
        );
    }

    #[tokio::test]
    async fn data_port_collision_is_explained() {
        let index = crate::iface::netinfo::find("lo0")
            .unwrap()
            .expect("lo0")
            .index;
        let held = std::net::UdpSocket::bind("[::1]:0").unwrap();
        let cfg = AutoConfig {
            discovery_port: free_v6_port(),
            data_port: held.local_addr().unwrap().port(),
            ..AutoConfig::default()
        };
        let adopted = vec![Adopted {
            name: "lo0".into(),
            index,
            link_local: ip("::1"),
            all: vec![ip("::1")],
        }];
        let ep = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
            &[0xc1; 64],
        )));
        let error = run::attach_adopted(&ep, cfg, adopted, false).err().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        assert!(error.to_string().contains("rnsd"), "{error}");
    }
}
