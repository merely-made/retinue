//! End to end on lo0, which Darwin gives both `::1` and `fe80::1`; Linux's lo has no
//! link-local.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use std::io;
use std::net::{Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::sockets::{multicast, scoped};
use super::*;
use crate::auto::{descope, discovery_group, token_valid};
use crate::destination::DestinationName;
use crate::endpoint::Endpoint;
use crate::identity::PrivateIdentity;
use crate::iface::netinfo;

fn ip(text: &str) -> Ipv6Addr {
    text.parse().unwrap()
}

fn free_v6_port() -> u16 {
    UdpSocket::bind("[::1]:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn lo0() -> u32 {
    netinfo::find("lo0").unwrap().expect("lo0").index
}

fn adopt(addr: &str) -> Vec<Adopted> {
    vec![Adopted {
        name: "lo0".into(),
        index: lo0(),
        link_local: ip(addr),
        all: vec![ip(addr)],
    }]
}

fn endpoint(seed: u8) -> Arc<Endpoint> {
    Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[seed; 64],
    )))
}

fn ports(group: &[u8]) -> AutoConfig {
    AutoConfig {
        group_id: group.to_vec(),
        discovery_port: free_v6_port(),
        data_port: free_v6_port(),
        ..AutoConfig::default()
    }
}

async fn eventually(mut done: impl FnMut() -> bool, tries: u32) -> bool {
    for _ in 0..tries {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    done()
}

fn bindable(addr: Ipv6Addr, port: u16) -> bool {
    UdpSocket::bind(scoped(addr, port, lo0())).is_ok()
}

/// Two endpoints on lo0, one adopting `::1` and one `fe80::1`. Darwin drops lo0 multicast
/// from `fe80::1`, so only one side's tokens multicast: the other must learn it by the
/// reverse unicast token, and data then flows both ways.
#[tokio::test]
async fn peers_and_carries_on_loopback() {
    let cfg = AutoConfig {
        ifac: Some(Ifac::for_stream(Some("auto-unit"), None).unwrap()),
        ..ports(b"retinue-unit")
    };
    let (a, b) = (endpoint(0xa1), endpoint(0xb1));
    let ha = attach::attach_adopted(&a, cfg.clone(), adopt("::1"), None).unwrap();
    let hb = attach::attach_adopted(&b, cfg, adopt("fe80::1"), None).unwrap();

    let peered = |h: &AutoHandle, addr: &str| h.status().peers.iter().any(|p| p.addr == ip(addr));
    eventually(|| peered(&ha, "fe80::1") && peered(&hb, "::1"), 50).await;
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
        let fact = tokio::time::timeout(Duration::from_secs(5), to.next_announcement())
            .await
            .expect("announce crosses the peer interface")
            .unwrap();
        assert_eq!(fact.app_data, [seed]);
    }
    assert!(ha.status().counters.rx > 0 && hb.status().counters.rx > 0);

    let peer_iface = hb.status().peers[0].interface;
    drop(hb);
    let detached = eventually(|| !b.interface_ids().contains(&peer_iface), 50).await;
    assert!(detached, "closing detaches peers");
}

#[tokio::test]
async fn data_port_collision_is_explained() {
    let held = UdpSocket::bind("[::1]:0").unwrap();
    let cfg = AutoConfig {
        data_port: held.local_addr().unwrap().port(),
        ..ports(b"retinue-unit")
    };
    let error = attach::attach_adopted(&endpoint(0xc1), cfg, adopt("::1"), None)
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    assert!(error.to_string().contains("rnsd"), "{error}");
}

/// A closed endpoint stops its AutoInterface though the handle lives on, freeing the data
/// port, and refuses a new one.
#[tokio::test]
async fn closing_the_endpoint_stops_discovery() {
    let cfg = ports(b"retinue-close");
    let ep = endpoint(0xd1);
    let handle = attach::attach_adopted(&ep, cfg.clone(), adopt("::1"), None).unwrap();
    assert!(
        !bindable(ip("::1"), cfg.data_port),
        "the carrier holds the port"
    );
    ep.close();
    let freed = eventually(|| bindable(ip("::1"), cfg.data_port), 20).await;
    assert!(freed, "closing the endpoint stops the carrier");
    let error = attach::attach_adopted(&ep, cfg, adopt("::1"), None)
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    drop(handle);
}

static FAKE_LINK_LOCALS: Mutex<Vec<Ipv6Addr>> = Mutex::new(Vec::new());

fn fake_link_locals(_: &str) -> Option<Vec<Ipv6Addr>> {
    Some(FAKE_LINK_LOCALS.lock().unwrap().clone())
}

/// When the interface's link-local changes, the next job rebinds all three sockets to the
/// new one: the old data port is released and tokens are hashed over the new address.
#[tokio::test]
async fn follows_a_link_local_change() {
    let cfg = ports(b"retinue-follow");
    *FAKE_LINK_LOCALS.lock().unwrap() = vec![ip("fe80::1")];
    let ep = endpoint(0xe1);
    let handle =
        attach::attach_adopted(&ep, cfg.clone(), adopt("fe80::1"), Some(fake_link_locals)).unwrap();
    let group = discovery_group(&cfg.group_id, cfg.scope, cfg.addr_type);
    let listener = multicast(group, lo0(), cfg.discovery_port).unwrap();
    assert!(!bindable(ip("fe80::1"), cfg.data_port));

    *FAKE_LINK_LOCALS.lock().unwrap() = vec![ip("::1")];
    let moved = || handle.status().adopted[0].link_local == ip("::1");
    assert!(eventually(moved, 60).await, "{:?}", handle.status());
    assert!(
        bindable(ip("fe80::1"), cfg.data_port),
        "the old data socket is closed"
    );
    assert!(!bindable(ip("::1"), cfg.data_port), "the new one is bound");

    let mut buf = [0u8; 64];
    let token = async {
        loop {
            let (n, src) = listener.recv_from(&mut buf).await.unwrap();
            let SocketAddr::V6(src) = src else { continue };
            let src = descope(*src.ip());
            if src == ip("::1") && token_valid(&cfg.group_id, &src, &buf[..n]) {
                return src;
            }
        }
    };
    let from = tokio::time::timeout(Duration::from_secs(5), token).await;
    assert_eq!(
        from.ok(),
        Some(ip("::1")),
        "tokens come from the new address"
    );
}
