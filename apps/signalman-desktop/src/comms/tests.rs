use super::*;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use signalman::comms_adapter::comms::{DeliveryQueueReason, DeliveryStatus, Direction, Draft};
use signalman::message::{ApplyOutcome, MessageStatus, MessageTransport, incoming_event};
use std::sync::Arc;
use std::time::Duration;

fn local(identity: &PrivateIdentity) -> MessagePeer {
    MessagePeer::new(
        *outrider::delivery_destination(identity.public()).as_bytes(),
        Some(*identity.public().ed25519_bytes()),
    )
}

#[tokio::test]
async fn real_lxmf_delivery_reaches_the_shared_comms_journal_and_reopens_without_duplicates() {
    let directory = tempfile::tempdir().unwrap();
    let alice_key = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
    let bob_key = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let (alice, bob) = (local(&alice_key), local(&bob_key));
    let a = MessageStore::open(directory.path().join("alice.redb"), "alice").unwrap();
    let b = MessageStore::open(directory.path().join("bob.redb"), "bob").unwrap();
    let model = model(alice, a.clone(), None, Some(100));
    let thread = LxmfAdapter::<Mailbox>::conversation_id(alice, bob);
    let mut draft = Draft::reply_to(thread.clone());
    draft.body = "hello across the actual LXMF codec".into();
    let id = model.send(&draft).await.unwrap();
    assert_eq!(
        model.delivery_status(&thread, &id).await.unwrap(),
        DeliveryStatus::Queued(DeliveryQueueReason::Offline)
    );
    let outgoing = a.records().next().unwrap().message;
    let sender = Arc::new(Endpoint::with_network_stamp_policy(
        alice_key.clone(),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let receiver = Arc::new(Endpoint::with_network_stamp_policy(
        bob_key.clone(),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    connect(&sender, &receiver, LossModel::new(31), LossModel::new(42));
    outrider::register_delivery(
        &sender,
        &outrider::DeliveryAnnounce::named(b"Alice".to_vec()),
    )
    .unwrap();
    outrider::register_delivery(
        &receiver,
        &outrider::DeliveryAnnounce::named(b"Bob".to_vec()),
    )
    .unwrap();
    let announce = tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .unwrap()
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), receiver.next_announcement())
        .await
        .unwrap()
        .unwrap();
    let receive = tokio::spawn({
        let receiver = receiver.clone();
        async move {
            let accepted = receiver.accept_resource().await.unwrap();
            outrider::receive_direct_with_stamp_cost(
                &receiver,
                accepted,
                &outrider::DeliveredCache::default(),
                outrider::DEFAULT_MAX_MESSAGE_BYTES,
                None,
            )
            .await
            .unwrap()
        }
    });
    let payload = outgoing.encode_payload(0.1).unwrap();
    let sent =
        outrider::send_direct_stamped(&sender, &alice_key, &announce, &payload, [0; 32], 100_000)
            .await
            .unwrap();
    let observed_mode = sent.mode;
    let received = tokio::time::timeout(Duration::from_secs(10), receive)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.verification, outrider::Verification::Verified);
    assert_eq!(received.source_identity, Some(*alice_key.public()));
    let event = postilion::Event::authenticated_message(received);
    let incoming = incoming_event(&event, bob, 101).unwrap();
    assert_eq!(b.append(incoming.clone()).unwrap(), ApplyOutcome::Applied);
    assert_eq!(b.append(incoming).unwrap(), ApplyOutcome::Duplicate);
    a.append(signalman::message::sent_event(
        outgoing.id(),
        &postilion::Sent::handed_to_radio(sent),
        102,
    ))
    .unwrap();
    assert_eq!(
        model.delivery_status(&thread, &id).await.unwrap(),
        DeliveryStatus::HandedToRadio
    );
    assert_eq!(
        a.records().next().unwrap().status,
        MessageStatus::HandedToRadio {
            transport_id: match event {
                postilion::Event::Message { message_id, .. } => message_id,
                _ => panic!("authenticated message"),
            },
            mode: MessageTransport::from(observed_mode)
        }
    );
    drop(model);
    drop(a);
    drop(b);
    let reopened = MessageStore::open(directory.path().join("bob.redb"), "bob").unwrap();
    let incoming_model = super::model(bob, reopened.clone(), None, None);
    let inbox = incoming_model.inbox().await;
    assert!(inbox.failures.is_empty());
    assert_eq!(inbox.conversations.len(), 1);
    let messages = incoming_model
        .messages(&inbox.conversations[0].id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, id);
    assert_eq!(messages[0].direction, Direction::Incoming);
    assert_eq!(messages[0].body.text(), draft.body);
    assert_eq!(
        incoming_model
            .delivery_status(&inbox.conversations[0].id, &id)
            .await
            .unwrap(),
        DeliveryStatus::ReceivedDirect
    );
    assert!(
        reopened.contacts().is_empty(),
        "receipt does not silently save the sender"
    );
    let wrong_local = MessagePeer::new([9; 16], Some([9; 32]));
    assert!(incoming_event(&event, wrong_local, 104).is_err());
    let wrong_model = super::model(wrong_local, reopened, None, None);
    assert!(wrong_model.inbox().await.conversations.is_empty());
}

#[test]
fn concurrent_clones_serialize_queue_intents_without_id_aliasing() {
    let store = MessageStore::memory("shared");
    let local = MessagePeer::new([1; 16], Some([1; 32]));
    let remote = MessagePeer::new([2; 16], None);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .queue_text_at(local, remote, "same text, same clock", 100)
                    .unwrap()
            })
        })
        .collect();
    let ids: std::collections::HashSet<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(ids.len(), 8);
    assert_eq!(store.len(), 8);
}

#[tokio::test]
async fn subject_refusal_does_not_write_the_durable_journal() {
    let store = MessageStore::memory("scope");
    let local = MessagePeer::new([1; 16], Some([1; 32]));
    let remote = MessagePeer::new([2; 16], None);
    let model = model(local, store.clone(), None, Some(100));
    let mut draft = Draft::reply_to(LxmfAdapter::<Mailbox>::conversation_id(local, remote));
    draft.body = "body".into();
    draft.subject = Some("unsupported".into());
    assert!(matches!(
        model.send(&draft).await,
        Err(comms::AdapterError::Unsupported(_))
    ));
    assert_eq!(store.log_len(), 0);
}

#[test]
fn face_queue_uses_comms_and_releases_handoff_only_after_a_valid_journal_outcome() {
    let mut state = crate::state::DesktopState::new(&crate::default_catalog_path());
    let local = MessagePeer::new([1; 16], Some([1; 32]));
    let (sender, mut receiver) = crate::station::test_channel();
    state.station_sender = Some(sender.clone());
    state.set_message_local(local);
    state.message_recipient = comms_text_input(&hex::encode([2; 16]));
    state.message_draft = comms_text_input("from the actual face");
    state.queue_message_at(100);
    let command = receiver.try_recv().unwrap();
    assert_eq!(state.selected_message, Some(command.id()));
    assert_eq!(state.message_store.len(), 1);
    assert_eq!(sender.pending_count().unwrap(), 1);
    state.apply_station_event(crate::station::StationEvent::Message {
        event: signalman::message::MessageEvent::StatusChanged {
            id: MessageId([9; 32]),
            status: MessageStatus::Failed("unknown intent".into()),
            observed_unix_ms: 101,
        },
        complete: true,
    });
    assert_eq!(
        sender.pending_count().unwrap(),
        1,
        "a refused event cannot acknowledge capacity"
    );
    state.apply_station_event(crate::station::StationEvent::Message {
        event: signalman::message::MessageEvent::StatusChanged {
            id: command.id(),
            status: MessageStatus::Queued(signalman::message::QueuedReason::WaitingForPeer),
            observed_unix_ms: 102,
        },
        complete: true,
    });
    assert_eq!(sender.pending_count().unwrap(), 0);
    state.dispatch_queued_messages(None);
    assert!(receiver.try_recv().is_err());
    state.dispatch_queued_messages(Some(MessagePeer::new([3; 16], None)));
    assert!(receiver.try_recv().is_err());
    state.dispatch_queued_messages(Some(MessagePeer::new([2; 16], None)));
    assert_eq!(receiver.try_recv().unwrap().id(), command.id());
}

fn comms_text_input(text: &str) -> cambium::TextInput {
    cambium::TextInput::new(text)
}

#[test]
fn reopened_history_supplies_offline_authoring_but_never_selects_between_station_identities() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.redb");
    let local = MessagePeer::new([1; 16], Some([1; 32]));
    let remote = MessagePeer::new([2; 16], None);
    {
        let store = MessageStore::open(&path, "local").unwrap();
        store
            .queue_text_at(local, remote, "saved intent", 100)
            .unwrap();
    }
    let mut state = crate::state::DesktopState::new(&crate::default_catalog_path());
    state.replace_message_store(MessageStore::open(&path, "local").unwrap());
    assert_eq!(state.message_local, Some(local));
    assert!(state.station_sender.is_none());
    state.message_recipient = comms_text_input(&hex::encode(remote.destination));
    state.message_draft = comms_text_input("another offline intent");
    state.queue_message_at(101);
    assert_eq!(state.message_store.len(), 2);
    assert!(state.message_store.records().all(|record| matches!(
        record.status,
        MessageStatus::Queued(signalman::message::QueuedReason::Offline)
    )));
    state
        .message_store
        .queue_text_at(
            MessagePeer::new([3; 16], Some([3; 32])),
            remote,
            "different station",
            102,
        )
        .unwrap();
    assert_eq!(state.message_store.known_local_peer(), None);
}

#[test]
fn companion_identity_events_use_one_adapter_per_protocol_and_disconnect_preserves_history() {
    use signalman::radio_comms::{CompanionScope, FileJournal, RadioComms};
    let dir = tempfile::tempdir().unwrap();
    let mt = RadioComms::open_companion(
        CompanionScope::Meshtastic {
            realm: "stock".into(),
            node: 1,
            channel: 0,
        },
        FileJournal::new(dir.path().join("mt.json")),
    )
    .unwrap();
    let mc = RadioComms::open_companion(
        CompanionScope::MeshCore {
            realm: "stock".into(),
            key: [7; 32],
        },
        FileJournal::new(dir.path().join("mc.json")),
    )
    .unwrap();
    let mut state = crate::state::DesktopState::new(&crate::default_catalog_path());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.apply_companion_event(crate::companion::CompanionEvent::Connected {
        protocol: comms::ProtocolKind::Meshtastic,
        owner: mt.clone(),
        stop: stop.clone(),
        port: "sim-mt".into(),
        source: "test companion",
        firmware: None,
    });
    state.apply_companion_event(crate::companion::CompanionEvent::Connected {
        protocol: comms::ProtocolKind::MeshCore,
        owner: mc.clone(),
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        port: "sim-mc".into(),
        source: "test companion",
        firmware: None,
    });
    assert_eq!(state.comms().unwrap().identities().len(), 2);
    state.select_message_protocol(comms::ProtocolKind::Meshtastic);
    state.message_recipient = comms_text_input("!00000002");
    state.message_draft = comms_text_input("from stock page");
    state.queue_message();
    assert_eq!(mt.records().unwrap().len(), 1);
    assert!(mc.records().unwrap().is_empty());
    state.disconnect_companion();
    assert!(stop.load(std::sync::atomic::Ordering::Relaxed));
    state.apply_companion_event(crate::companion::CompanionEvent::Stopped {
        protocol: comms::ProtocolKind::Meshtastic,
        detail: "disconnected".into(),
    });
    assert_eq!(
        state
            .radio_owner(comms::ProtocolKind::Meshtastic)
            .unwrap()
            .records()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn companion_connection_controls_validate_before_dispatch_and_allow_explicit_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = crate::state::DesktopState::new(&crate::default_catalog_path());
    state.select_message_protocol(comms::ProtocolKind::Meshtastic);
    state.request_companion_connection();
    assert!(state.take_companion_request().is_none());
    state.companion_port = comms_text_input("/dev/cu.stock");
    state.companion_realm = comms_text_input("household");
    state.companion_journal = comms_text_input(dir.path().join("radio.json").to_str().unwrap());
    state.request_companion_connection();
    let first = state.take_companion_request().unwrap();
    assert_eq!(first.realm, "household");
    state.request_companion_connection();
    assert!(state.take_companion_request().is_none());
    state.apply_companion_event(crate::companion::CompanionEvent::Stopped {
        protocol: comms::ProtocolKind::Meshtastic,
        detail: "simulated disconnect".into(),
    });
    state.request_companion_connection();
    assert!(state.take_companion_request().is_some());
    state.select_message_protocol(comms::ProtocolKind::MeshCore);
    state.request_companion_connection();
    assert!(state.take_companion_request().is_none()); // same serial port/journal already claimed
}
#[test]
fn companion_settings_refuse_port_aliases_and_separate_scope_paths() {
    use crate::companion::{CompanionSettings, validate_settings};
    use signalman::radio_comms::companion::CompanionProtocol;
    let mt = CompanionSettings {
        protocol: CompanionProtocol::Meshtastic { channel: 0 },
        port: "/dev/cu.radio".into(),
        realm: "house".into(),
        journal: "/tmp/mt-test.json".into(),
    };
    let mut mc = CompanionSettings {
        protocol: CompanionProtocol::MeshCore,
        port: "/dev/tty.radio".into(),
        realm: "house".into(),
        journal: "/tmp/mc-test.json".into(),
    };
    assert!(validate_settings(&[mt.clone(), mc.clone()], None).is_err());
    mc.port = "/dev/cu.other".into();
    assert!(validate_settings(&[mt.clone(), mc.clone()], None).is_ok());
    assert!(
        validate_settings(
            std::slice::from_ref(&mt),
            Some(std::path::Path::new("/dev/tty.radio"))
        )
        .is_err()
    );
    mc.journal = mt.journal.clone();
    assert!(validate_settings(&[mt, mc], None).is_err());
}
#[test]
fn invalid_companion_configuration_is_rejected_before_opening_serial_io() {
    let worker = crate::companion::CompanionWorker::spawn(
        crate::companion::CompanionSettings {
            protocol: signalman::radio_comms::companion::CompanionProtocol::MeshCore,
            port: "/dev/nonexistent-stock-companion".into(),
            realm: "invalid realm".into(),
            journal: "/tmp/not-created-stock-companion.json".into(),
        },
        std::sync::Arc::new(|| {}),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(event) = worker.drain().into_iter().next() {
            let crate::companion::CompanionEvent::Stopped { detail, .. } = event else {
                panic!("invalid configuration cannot connect")
            };
            assert!(detail.contains("Realm must contain"));
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
