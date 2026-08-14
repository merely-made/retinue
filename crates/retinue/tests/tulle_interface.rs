use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use retinue::Ifac;
use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::iface::tulle::drive;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};
use tulle::link::Received;
use tulle::radio_io::PacketRadio;
use tulle::serial::TransmitError;

struct MemoryRadio {
    peer: mpsc::UnboundedSender<Vec<u8>>,
    inbound: mpsc::UnboundedReceiver<Vec<u8>>,
    sent: Arc<Mutex<VecDeque<Vec<u8>>>>,
    announcement_sends: Arc<AtomicUsize>,
    notify: Arc<Notify>,
    max_frame_len: usize,
}

fn radio_pair(max_frame_len: usize) -> (MemoryRadio, MemoryRadio) {
    let (a_tx, a_rx) = mpsc::unbounded_channel();
    let (b_tx, b_rx) = mpsc::unbounded_channel();
    let sent = Arc::new(Mutex::new(VecDeque::new()));
    let announcement_sends = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());
    (
        MemoryRadio {
            peer: b_tx,
            inbound: a_rx,
            sent: Arc::clone(&sent),
            announcement_sends: Arc::clone(&announcement_sends),
            notify: Arc::clone(&notify),
            max_frame_len,
        },
        MemoryRadio {
            peer: a_tx,
            inbound: b_rx,
            sent,
            announcement_sends,
            notify,
            max_frame_len,
        },
    )
}

// Mirrors the trait's explicit `impl Future + Send`, as tulle's own impls do.
#[allow(clippy::manual_async_fn)]
impl PacketRadio for MemoryRadio {
    fn max_frame_len(&self) -> usize {
        self.max_frame_len
    }

    fn send_frame(
        &self,
        frame: Vec<u8>,
    ) -> impl Future<Output = Result<Duration, TransmitError>> + Send {
        let peer = self.peer.clone();
        let sent = Arc::clone(&self.sent);
        let notify = Arc::clone(&self.notify);
        async move {
            sent.lock().unwrap().push_back(frame.clone());
            notify.notify_waiters();
            peer.send(frame).map_err(|_| TransmitError::Stopped)?;
            Ok(Duration::from_millis(1))
        }
    }

    fn send_announcement(
        &self,
        frame: Vec<u8>,
    ) -> impl Future<Output = Result<Duration, TransmitError>> + Send {
        let peer = self.peer.clone();
        let sent = Arc::clone(&self.sent);
        let announcement_sends = Arc::clone(&self.announcement_sends);
        let notify = Arc::clone(&self.notify);
        async move {
            announcement_sends.fetch_add(1, Ordering::Relaxed);
            sent.lock().unwrap().push_back(frame.clone());
            notify.notify_waiters();
            peer.send(frame).map_err(|_| TransmitError::Stopped)?;
            Ok(Duration::from_millis(1))
        }
    }

    fn recv_frame(&mut self) -> impl Future<Output = Option<Received>> + Send {
        async move {
            self.inbound.recv().await.map(|frame| Received {
                frame,
                rssi_dbm: -50,
                snr_db: 8.0,
            })
        }
    }
}

#[tokio::test]
async fn endpoint_announces_cross_the_tulle_packet_boundary() {
    let alice = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x11; 64]));
    let bob_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let bob = Endpoint::new(bob_id.clone());
    let (alice_radio, bob_radio) = radio_pair(500);
    let bob_announcement_sends = Arc::clone(&bob_radio.announcement_sends);

    let alice_task = tokio::spawn(drive(alice.attach_interface(), alice_radio));
    let bob_task = tokio::spawn(drive(bob.attach_interface(), bob_radio));

    let name = DestinationName::new("bench", ["radio"]);
    let destination = name.destination_hash(bob_id.public());
    bob.announce(&name, b"tulle interface");

    let announce = tokio::time::timeout(Duration::from_secs(1), alice.next_announcement())
        .await
        .expect("announce crossed the radio")
        .expect("endpoint remains live");
    assert_eq!(announce.destination, destination);
    assert_eq!(announce.app_data, b"tulle interface");
    assert_eq!(
        bob_announcement_sends.load(Ordering::Relaxed),
        1,
        "the Retinue bridge routed the packet through Tulle's announce path"
    );

    alice_task.abort();
    bob_task.abort();
}

#[tokio::test]
async fn physical_frame_limit_is_applied_before_the_driver_runs() {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x33; 64]));
    let (radio, _peer) = radio_pair(20);
    let interface = endpoint.attach_interface();
    let task = tokio::spawn(drive(interface, radio));

    endpoint.announce(
        &DestinationName::new("bench", ["oversize"]),
        b"larger than cap",
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !task.is_finished(),
        "carrier admission must refuse the packet before it can stop the driver"
    );
    task.abort();
}

#[tokio::test]
async fn negotiated_radio_mtu_chunks_a_best_effort_stream() {
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x44; 64]));
    let server_id = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    client.set_link_mtu(255);
    server.set_link_mtu(255);

    let name = DestinationName::new("bench", ["radio-stream"]);
    let destination = name.destination_hash(server_id.public());
    server.register(name.clone(), b"");

    let (client_radio, server_radio) = radio_pair(255);
    let client_task = tokio::spawn(drive(client.attach_interface(), client_radio));
    let server_task = tokio::spawn(drive(server.attach_interface(), server_radio));
    server.announce(&name, b"");

    tokio::time::timeout(Duration::from_secs(2), client.next_announcement())
        .await
        .expect("announce crossed the capped radio")
        .expect("client remains live");

    let expected: Vec<u8> = (0..1_024u32).map(|n| (n * 17 + 3) as u8).collect();
    let expected_server = expected.clone();
    let server_for_accept = Arc::clone(&server);
    let accept = tokio::spawn(async move {
        let mut stream = server_for_accept.accept().await.expect("accept stream");
        let mut received = vec![0u8; expected_server.len()];
        stream
            .read_exact(&mut received)
            .await
            .expect("read chunked stream");
        assert_eq!(received, expected_server);
        stream.write_all(b"ok").await.expect("write reply");
        stream.flush().await.expect("flush reply");
    });

    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        client.open(destination, *server_id.public()),
    )
    .await
    .expect("link setup timed out")
    .expect("open stream");
    stream.write_all(&expected).await.expect("write 1 KiB");
    stream.flush().await.expect("flush 1 KiB");
    let mut reply = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
        .await
        .expect("reply timed out")
        .expect("read reply");
    assert_eq!(&reply, b"ok");
    accept.await.expect("accept task");

    assert!(
        !client_task.is_finished() && !server_task.is_finished(),
        "a 255-byte radio driver must stay live for a negotiated 255-byte link"
    );
    client_task.abort();
    server_task.abort();
}

#[tokio::test]
async fn negotiated_ifac_radio_mtu_keeps_resource_parts_within_frame() {
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x66; 64]));
    let server_id = PrivateIdentity::from_secret_bytes(&[0x77; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    client.set_link_mtu(247);
    server.set_link_mtu(247);

    let access = Ifac::new(Some("retinue-test"), Some("ifac-resource"), 8).unwrap();
    let (client_radio, server_radio) = radio_pair(255);
    let client_interface = client
        .attach_interface_with_ifac(255, access.clone())
        .unwrap();
    let server_interface = server.attach_interface_with_ifac(255, access).unwrap();
    let client_task = tokio::spawn(drive(client_interface, client_radio));
    let server_task = tokio::spawn(drive(server_interface, server_radio));

    let name = DestinationName::new("bench", ["ifac-resource"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"247 logical plus 8 IFAC");
    let announce = tokio::time::timeout(Duration::from_secs(1), client.next_announcement())
        .await
        .expect("IFAC announce crossed the capped radio")
        .expect("client remains live");
    assert_eq!(announce.destination, destination);

    let expected: Vec<u8> = (0..286_u32).map(|n| (n * 29 + 7) as u8).collect();
    let expected_server = expected.clone();
    let transfer = ResourceTransferConfig {
        timeout: Duration::from_secs(2),
        retry_interval: Duration::from_millis(20),
        request_window: 1,
    };
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.expect("accept Resource");
            accepted.session.set_config(transfer);
            accepted.session.fetch().await.expect("receive Resource")
        }
    });
    client
        .publish_resource_with_config(destination, *server_id.public(), &expected, transfer)
        .await
        .expect("publish Resource");
    assert_eq!(receiver.await.expect("receiver task"), expected_server);
    assert!(
        !client_task.is_finished() && !server_task.is_finished(),
        "247-byte logical Resource frames plus IFAC must fit the 255-byte carrier"
    );

    client_task.abort();
    server_task.abort();
}
