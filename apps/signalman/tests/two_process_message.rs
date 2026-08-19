use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use outrider::{
    DeliveryAnnounce, LxmfPayload, announce_delivery, delivery_destination,
    receive_direct_with_stamp_cost, register_delivery, send_direct_stamped,
};
use postilion::{Event, Sent};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use signalman::message::{
    MessageBook, MessageEvent, MessagePeer, MessageStatus, QueuedReason, TextMessage, WIRE_TITLE,
    incoming_event, sent_event,
};

const ROLE_ENV: &str = "SIGNALMAN_TWO_PROCESS_ROLE";
const DIR_ENV: &str = "SIGNALMAN_TWO_PROCESS_DIR";
const ADDR_ENV: &str = "SIGNALMAN_TWO_PROCESS_ADDR";

#[test]
fn child_process_entry() {
    let Ok(role) = std::env::var(ROLE_ENV) else {
        return;
    };
    let dir = PathBuf::from(std::env::var_os(DIR_ENV).expect("fixture directory"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    match role.as_str() {
        "receiver" => runtime.block_on(receiver(&dir)),
        "sender" => {
            let address = std::env::var(ADDR_ENV).unwrap().parse().unwrap();
            runtime.block_on(sender(&dir, address));
        }
        other => panic!("unknown child role {other}"),
    }
}

async fn receiver(dir: &Path) {
    progress(dir, "receiver", "starting");
    let identity = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let endpoint = Arc::new(Endpoint::new(identity.clone()));
    let announce = DeliveryAnnounce::named(b"Receiver".to_vec());
    register_delivery(&endpoint, &announce).unwrap();
    let bound = endpoint
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    std::fs::write(dir.join("ready"), bound.to_string()).unwrap();
    progress(dir, "receiver", "listening");

    let announcements = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let announce = announce.clone();
        async move {
            loop {
                let _ = announce_delivery(&endpoint, &announce);
                tokio::time::sleep(Duration::from_millis(75)).await;
            }
        }
    });

    let accepted = tokio::time::timeout(Duration::from_secs(12), endpoint.accept_resource())
        .await
        .expect("sender opens a delivery session")
        .unwrap();
    progress(dir, "receiver", "accepted");
    let received = receive_direct_with_stamp_cost(&endpoint, accepted, 64 * 1024, None)
        .await
        .unwrap();
    progress(dir, "receiver", "authenticated");
    let event = Event::authenticated_message(received);
    let local = MessagePeer::new(
        *delivery_destination(identity.public()).as_bytes(),
        Some(*identity.public().ed25519_bytes()),
    );
    let event = incoming_event(&event, local, 200).unwrap();
    let app_id = event.message_id();
    let (transport_id, text) = match &event {
        MessageEvent::IncomingReceived {
            message,
            transport_id,
            ..
        } => (*transport_id, message.text().unwrap().to_owned()),
        _ => unreachable!(),
    };
    let mut book = MessageBook::default();
    book.apply(&event).unwrap();
    let record = book.get(app_id).unwrap();
    write_receipt(
        &dir.join("received"),
        app_id.0,
        transport_id,
        record.status.label(),
        &text,
    );
    progress(dir, "receiver", "complete");
    // The receiver has its application receipt, while the sender still needs
    // Retinue's final link acknowledgement before its handoff future returns.
    tokio::time::sleep(Duration::from_millis(750)).await;
    announcements.abort();
}

async fn sender(dir: &Path, address: std::net::SocketAddr) {
    progress(dir, "sender", "starting");
    let identity = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
    let receiver_identity = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let endpoint = Arc::new(Endpoint::new(identity.clone()));
    endpoint.attach_tcp_client(address).await.unwrap();
    progress(dir, "sender", "connected");
    let announce = DeliveryAnnounce::named(b"Sender".to_vec());
    register_delivery(&endpoint, &announce).unwrap();
    for _ in 0..3 {
        announce_delivery(&endpoint, &announce).unwrap();
        tokio::time::sleep(Duration::from_millis(75)).await;
    }
    let receiver_announce = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let heard = endpoint.next_announcement().await.unwrap();
            if heard.identity == *receiver_identity.public() {
                break heard;
            }
        }
    })
    .await
    .expect("receiver announces");
    progress(dir, "sender", "discovered");

    let sender = MessagePeer::new(
        *delivery_destination(identity.public()).as_bytes(),
        Some(*identity.public().ed25519_bytes()),
    );
    let recipient = MessagePeer::new(
        *delivery_destination(receiver_identity.public()).as_bytes(),
        Some(*receiver_identity.public().ed25519_bytes()),
    );
    let message = TextMessage::compose(sender, recipient, 100, [0x77; 32], "two process hello");
    let app_id = message.id;
    let queued = MessageEvent::OutgoingQueued {
        message: message.clone().into(),
        reason: QueuedReason::Offline,
        observed_unix_ms: 101,
    };
    let mut book = MessageBook::default();
    book.apply(&queued).unwrap();
    progress(dir, "sender", "persisted");

    let payload = LxmfPayload::text(1.0, WIRE_TITLE, message.encode_wire().unwrap());
    let direct = tokio::time::timeout(
        Duration::from_secs(12),
        send_direct_stamped(
            &endpoint,
            &identity,
            &receiver_announce,
            &payload,
            [0; 32],
            0,
        ),
    )
    .await
    .expect("transport returns")
    .unwrap();
    progress(dir, "sender", "transport-returned");
    let sent = Sent::handed_to_radio(direct);
    let status = sent_event(app_id, &sent, 102);
    let transport_id = match &status {
        MessageEvent::StatusChanged {
            status: MessageStatus::HandedToRadio { transport_id, .. },
            ..
        } => *transport_id,
        _ => unreachable!(),
    };
    book.apply(&status).unwrap();
    write_receipt(
        &dir.join("sent"),
        app_id.0,
        transport_id,
        book.get(app_id).unwrap().status.label(),
        &message.text,
    );
    progress(dir, "sender", "complete");
}

fn write_receipt(path: &Path, app: [u8; 32], transport: [u8; 32], status: &str, text: &str) {
    std::fs::write(
        path,
        format!(
            "app={}\ntransport={}\nstatus={status}\ntext={text}\n",
            hex(&app),
            hex(&transport)
        ),
    )
    .unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn progress(dir: &Path, role: &str, stage: &str) {
    std::fs::write(dir.join(format!("{role}-progress")), stage).unwrap();
}

#[test]
fn two_process_text_exchange_matches_transport_and_conversation_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut receiver = child(&executable, "receiver", dir.path(), None);
    let ready = wait_for_file(
        dir.path().join("ready"),
        &mut receiver,
        "receiver readiness",
    );
    let mut sender = child(&executable, "sender", dir.path(), Some(ready.trim()));

    wait_success(&mut sender, "sender", dir.path());
    wait_success(&mut receiver, "receiver", dir.path());
    let sent = receipt(&dir.path().join("sent"));
    let received = receipt(&dir.path().join("received"));

    assert_eq!(sent["app"], received["app"]);
    assert_eq!(sent["transport"], received["transport"]);
    assert_eq!(sent["text"], "two process hello");
    assert_eq!(received["text"], "two process hello");
    assert_eq!(sent["status"], "handed to radio");
    assert_eq!(received["status"], "received directly");
}

fn child(executable: &Path, role: &str, dir: &Path, address: Option<&str>) -> Child {
    let mut command = Command::new(executable);
    command
        .args([
            "--exact",
            "child_process_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE_ENV, role)
        .env(DIR_ENV, dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if let Some(address) = address {
        command.env(ADDR_ENV, address);
    }
    command.spawn().unwrap()
}

fn wait_for_file(path: PathBuf, child: &mut Child, purpose: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(value) = std::fs::read_to_string(&path) {
            return value;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("child exited before {purpose}: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("timed out waiting for {purpose}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_success(child: &mut Child, name: &str, dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let sender = std::fs::read_to_string(dir.join("sender-progress"))
                .unwrap_or_else(|_| "unknown".into());
            let receiver = std::fs::read_to_string(dir.join("receiver-progress"))
                .unwrap_or_else(|_| "unknown".into());
            assert!(
                status.success(),
                "{name} child failed: {status}; sender={sender}, receiver={receiver}"
            );
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let progress = std::fs::read_to_string(dir.join(format!("{name}-progress")))
                .unwrap_or_else(|_| "unknown".into());
            let sender = std::fs::read_to_string(dir.join("sender-progress"))
                .unwrap_or_else(|_| "unknown".into());
            let receiver = std::fs::read_to_string(dir.join("receiver-progress"))
                .unwrap_or_else(|_| "unknown".into());
            panic!("{name} child timed out at {progress}; sender={sender}, receiver={receiver}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn receipt(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.to_owned(), value.to_owned())
        })
        .collect()
}
