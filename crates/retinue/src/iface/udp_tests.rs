use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use super::*;
use crate::destination::DestinationName;
use crate::identity::PrivateIdentity;

fn endpoint(seed: u8) -> Endpoint {
    Endpoint::new(PrivateIdentity::from_secret_bytes(&[seed; 64]))
}

fn free_port() -> u16 {
    std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn local(port: u16) -> Option<SocketAddrV4> {
    Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

async fn wait_for(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[test]
fn stock_size_bound_is_on_the_packet_after_the_code() {
    assert!(frame_fits(HW_MTU, HW_MTU, 0));
    assert!(!frame_fits(HW_MTU + 1, HW_MTU, 0));
    assert!(frame_fits(HW_MTU + 32, HW_MTU, 16));
    assert!(!frame_fits(HW_MTU + 33, HW_MTU, 16));
}

#[tokio::test]
async fn announces_cross_a_pair_with_stream_ifac() {
    let (pa, pb) = (free_port(), free_port());
    let (a, b) = (endpoint(1), endpoint(2));
    let ifac = || Some(Ifac::for_stream(Some("udp-test"), None).unwrap());
    let cfg_a = UdpConfig {
        listen: local(pa),
        forward: local(pb),
        ..UdpConfig::default()
    };
    let cfg_b = UdpConfig {
        listen: local(pb),
        forward: local(pa),
        ..UdpConfig::default()
    };
    let ha = a.attach_udp(cfg_a, ifac()).await.unwrap();
    let hb = b.attach_udp(cfg_b, ifac()).await.unwrap();
    let name = DestinationName::new("retinue", ["udp-test"]);
    a.register(name.clone(), b"over udp");
    a.announce(&name, b"over udp");
    let fact = tokio::time::timeout(Duration::from_secs(5), b.next_announcement())
        .await
        .expect("announce arrives")
        .unwrap();
    assert_eq!(fact.app_data, b"over udp");
    assert!(ha.counters().tx >= 1 && hb.counters().rx >= 1);
}

#[tokio::test]
async fn own_frames_heard_back_are_dropped() {
    let port = free_port();
    let ep = endpoint(3);
    let looped = UdpConfig {
        listen: local(port),
        forward: local(port),
        ..UdpConfig::default()
    };
    let handle = ep.attach_udp(looped, None).await.unwrap();
    let name = DestinationName::new("retinue", ["udp-echo"]);
    ep.register(name.clone(), b"");
    ep.announce(&name, b"");
    assert!(wait_for(|| handle.counters().own_echo > 0).await);
    assert_eq!(handle.counters().rx, 0);
}

#[tokio::test]
async fn oversize_and_foreign_frames() {
    let port = free_port();
    let ep = endpoint(4);
    let cfg = UdpConfig {
        listen: local(port),
        ..UdpConfig::default()
    };
    let handle = ep.attach_udp(cfg, None).await.unwrap();
    let raw = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    raw.send_to(&[0u8; HW_MTU + 1], (Ipv4Addr::LOCALHOST, port))
        .unwrap();
    raw.send_to(&[0u8; 40], (Ipv4Addr::LOCALHOST, port))
        .unwrap();
    assert!(
        wait_for(|| handle.counters()
            == UdpCounters {
                rx: 1,
                oversize: 1,
                ..UdpCounters::default()
            })
        .await
    );
}
