use alloc::vec;
use std::sync::{Arc, Mutex};

use tokio::io::{DuplexStream, duplex};

use super::*;
use crate::destination::DestinationName;
use crate::identity::PrivateIdentity;

fn endpoint(seed: u8) -> Endpoint {
    Endpoint::new(PrivateIdentity::from_secret_bytes(&[seed; 64]))
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

#[tokio::test]
async fn hdlc_and_kiss_carriers_exchange_announces() {
    for kiss in [false, true] {
        let framing = || match kiss {
            true => Framing::Kiss(KissTnc::new(kiss::TncConfig::default(), None)),
            false => Framing::Hdlc,
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
