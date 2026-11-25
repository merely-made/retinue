use alloc::vec;
use std::sync::{Arc, Mutex};

use tokio::io::{DuplexStream, duplex};

use super::*;
use crate::destination::DestinationName;
use crate::identity::PrivateIdentity;

fn endpoint(seed: u8) -> Endpoint {
    Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[seed; 64]),
        crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    )
}

/// Run a carrier for `ep` over the given streams, opened one per attempt.
fn spawn_carrier(ep: &Endpoint, streams: Vec<DuplexStream>, framing: Framing) -> Arc<Mutex<usize>> {
    let interface = ep.attach_interface_with_frame_limit(HW_MTU).unwrap();
    let (outbound, sink) = interface.split();
    let opened = Arc::new(Mutex::new(0));
    let count = Arc::clone(&opened);
    let mut streams = streams.into_iter();
    tokio::spawn(run(
        move || {
            *count.lock().unwrap() += 1;
            streams.next().ok_or_else(|| io::ErrorKind::NotFound.into())
        },
        outbound,
        sink,
        framing,
        watch::channel(CarrierStatus::Opening).0,
    ));
    opened
}

async fn announce_received(ep: &Endpoint) -> bool {
    tokio::time::timeout(Duration::from_secs(3), ep.next_announcement())
        .await
        .is_ok_and(|announce| announce.is_ok())
}

fn name() -> DestinationName {
    DestinationName::new("retinue", ["serial-test"])
}

fn n0call() -> Ax25Header {
    Ax25Header::new("N0CALL", 7).unwrap()
}

#[tokio::test]
async fn hdlc_kiss_and_ax25_carriers_exchange_announces() {
    for kiss in [0, 1, 2] {
        let framing = || match kiss {
            1 => Framing::Kiss(KissTnc::new(kiss::TncConfig::default(), None)),
            2 => Framing::ax25(n0call(), TncConfig::default()),
            _ => Framing::Hdlc,
        };
        let (a, b) = (endpoint(1), endpoint(2));
        let (left, right) = duplex(4096);
        spawn_carrier(&a, vec![left], framing());
        spawn_carrier(&b, vec![right], framing());
        tokio::time::sleep(Duration::from_millis(2_100)).await;
        a.announce(&name(), b"over serial");
        assert!(announce_received(&b).await, "kiss={kiss}");
    }
}

/// Capture the HDLC wire bytes of one announce from `ep`.
async fn announce_wire(ep: &Endpoint) -> Vec<u8> {
    let (left, mut right) = duplex(4096);
    spawn_carrier(ep, vec![left], Framing::Hdlc);
    tokio::time::sleep(Duration::from_millis(600)).await;
    ep.announce(&name(), b"wire");
    let mut wire = vec![0; 1024];
    let count = right.read(&mut wire).await.unwrap();
    wire.truncate(count);
    wire
}

#[tokio::test]
async fn a_stalled_partial_frame_is_discarded_after_100_ms() {
    let wire = announce_wire(&endpoint(3)).await;
    let receiver = endpoint(4);
    let (left, mut right) = duplex(4096);
    spawn_carrier(&receiver, vec![left], Framing::Hdlc);
    tokio::time::sleep(Duration::from_millis(600)).await;

    let (head, tail) = wire.split_at(wire.len() / 2);
    right.write_all(head).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    // The tail alone is junk until the next flag; the stalled head is gone.
    right.write_all(&tail[..tail.len() - 1]).await.unwrap();
    right.write_all(&[hdlc::FLAG]).await.unwrap();
    assert!(!announce_received(&receiver).await);

    right.write_all(&wire).await.unwrap();
    assert!(announce_received(&receiver).await);
}

#[tokio::test]
async fn a_failed_port_is_reopened_under_the_same_interface() {
    let (a, b) = (endpoint(5), endpoint(6));
    let (first, first_peer) = duplex(4096);
    let (second, second_peer) = duplex(4096);
    let opened = spawn_carrier(&a, vec![first, second], Framing::Hdlc);
    let ids = a.interface_ids();
    drop(first_peer);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(*opened.lock().unwrap(), 1);

    tokio::time::sleep(REOPEN + Duration::from_millis(700)).await;
    assert_eq!(*opened.lock().unwrap(), 2, "reopened after 5 s");
    assert_eq!(
        a.interface_ids(),
        ids,
        "the interface id survives the reopen"
    );
    spawn_carrier(&b, vec![second_peer], Framing::Hdlc);
    tokio::time::sleep(Duration::from_millis(600)).await;
    b.announce(&name(), b"after reopen");
    assert!(announce_received(&a).await);
}

#[tokio::test]
async fn kiss_writes_its_startup_after_the_settle() {
    let ep = endpoint(7);
    let (left, mut right) = duplex(4096);
    spawn_carrier(
        &ep,
        vec![left],
        Framing::Kiss(KissTnc::new(kiss::TncConfig::default(), None)),
    );
    let mut startup = vec![0; 64];
    let started = Instant::now();
    let count = right.read(&mut startup).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2));
    let expected = KissTnc::new(kiss::TncConfig::default(), None).startup();
    assert_eq!(&startup[..count], expected.as_slice());
}

#[test]
fn bad_line_settings_are_refused() {
    let config = SerialConfig {
        databits: 9,
        ..SerialConfig::new("/dev/null")
    };
    assert!(config.char_size().is_err());
    assert!(SerialConfig::new("/dev/null").stop_bits().is_ok());
}

#[tokio::test]
async fn ax25_frames_carry_the_header_and_headerless_frames_are_dropped() {
    let ep = endpoint(3);
    let (carrier, mut wire) = duplex(8192);
    spawn_carrier(
        &ep,
        vec![carrier],
        Framing::ax25(n0call(), TncConfig::default()),
    );
    tokio::time::sleep(Duration::from_millis(2_100)).await;

    // A peer's announce without the header is not AX.25 and goes nowhere.
    let peer = endpoint(4);
    let (mut out, _) = peer.attach_interface().split();
    peer.announce(&name(), b"ax25");
    let packet = out.recv().await.unwrap();
    let bytes = out.encode(&packet).unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    wire.write_all(&kiss::encode(kiss::cmd::DATA, &bytes))
        .await
        .unwrap();
    assert!(!announce_received(&ep).await);
    wire.write_all(&kiss::encode(kiss::cmd::DATA, &n0call().wrap(&bytes)))
        .await
        .unwrap();
    assert!(announce_received(&ep).await);

    ep.announce(&name(), b"from retinue");
    let mut buf = vec![0; 4096];
    let mut seen = Vec::new();
    while !seen
        .windows(18)
        .any(|w| w[..2] == [kiss::FEND, 0] && w[2..] == n0call().bytes()[..])
    {
        let n = tokio::time::timeout(Duration::from_secs(3), wire.read(&mut buf))
            .await
            .expect("an AX.25 frame")
            .unwrap();
        seen.extend_from_slice(&buf[..n]);
    }
}
