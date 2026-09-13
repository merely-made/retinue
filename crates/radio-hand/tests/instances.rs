use radio_hand::instances::{Config, Error, Event, RETINUE, Runtime, SENNET, TUCKET};
use retinue::{destination::DestinationName, identity::PrivateIdentity, node::Node};
use selvage::{
    PhyProfile,
    personality::{
        Acknowledgement, ControllerConfig, CoveragePolicy, Excursion, InstalledPersonalitySet,
        InterruptionPolicy,
    },
};
use sennet::{
    flood::{ManagedFloodConfig, RelayDelayWindow},
    instance::{PacketIdLease, SennetInstance, SennetInstanceConfig},
    node::Channel,
    node_info::NodeDirectoryConfig,
    transport::{ChannelKey, Header},
};
use tucket::{
    identity::LocalIdentity, instance::Instance as TucketInstance, node::Node as TucketNode,
};

fn runtime(tx_budget_ms: u64, frame_ttl_ms: u64) -> Runtime {
    let ids = [RETINUE, SENNET, TUCKET];
    let config = Config {
        controller: ControllerConfig {
            home: RETINUE,
            pin: None,
            installed: InstalledPersonalitySet::new(&ids).unwrap(),
            coverage: CoveragePolicy::AllowGap,
            max_excursion_ms: 1_000,
            return_budget_ms: 10,
            max_defer_ms: 100,
            transition_timeout_ms: 10,
        },
        profiles: [PhyProfile::meshtastic_long_fast(915_000_000); 3],
        tx_budget_ms,
        frame_ttl_ms,
    };
    let node = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[1; 64]),
        DestinationName::new("retinue", ["resident"]).name_hash(),
    );
    let channel = Channel {
        hash: 8,
        key: ChannelKey::Aes128([2; 16]),
    };
    let sennet = SennetInstance::new(
        SennetInstanceConfig {
            channel,
            flood: ManagedFloodConfig {
                channel_hash: 8,
                relay_node: 1,
                seen_capacity: 2,
                delay: RelayDelayWindow::new(
                    core::time::Duration::ZERO,
                    core::time::Duration::ZERO,
                )
                .unwrap(),
            },
            directory: NodeDirectoryConfig::default(),
            pending_ttl: 10,
        },
        PacketIdLease::new(1, 1, 8, 1).unwrap(),
    )
    .unwrap();
    let tucket = TucketInstance::new(
        TucketNode::new(LocalIdentity::from_seed([3; 32]), false),
        tucket::instance::InstanceConfig::new(2, 10).unwrap(),
    );
    Runtime::new(0, config, node, sennet, tucket).unwrap()
}

fn activate_sennet(runtime: &mut Runtime, now: u64) {
    let step = runtime
        .request(
            now,
            Excursion {
                target: SENNET,
                duration_ms: 20,
                interruption: InterruptionPolicy::ResumableOnly,
            },
            || [0; 16],
        )
        .unwrap();
    let transition = step.transition.expect("Sennet transition");
    runtime
        .acknowledge(now, transition.id, Acknowledgement::Completed)
        .unwrap();
}

fn activate(runtime: &mut Runtime, now: u64, target: selvage::personality::PersonalityId) {
    let step = runtime
        .request(
            now,
            Excursion {
                target,
                duration_ms: 20,
                interruption: InterruptionPolicy::ResumableOnly,
            },
            || [0; 16],
        )
        .unwrap();
    let transition = step.transition.expect("transition");
    runtime
        .acknowledge(now, transition.id, Acknowledgement::Completed)
        .unwrap();
}

fn header() -> Header {
    Header {
        destination: u32::MAX,
        source: 0,
        packet_id: 0,
        hop_limit: 3,
        want_ack: false,
        via_mqtt: false,
        hop_start: 3,
        channel_hash: 0,
        next_hop: 0,
        relay_node: 1,
    }
}

#[test]
fn an_exact_tx_deadline_is_not_physical_tx_budget() {
    let mut runtime = runtime(10, 11);
    activate_sennet(&mut runtime, 0);
    runtime.send_sennet(0, header(), "deadline").unwrap();

    // Sennet's pending deadline is now 10. Reserve time before the protocol
    // expiry boundary so the physical queue has a settlement margin.
    assert!(runtime.begin_tx(0).unwrap().is_none());
}

#[test]
fn stale_work_completion_is_refused_without_releasing_current_work() {
    let mut runtime = runtime(2, 20);
    activate_sennet(&mut runtime, 0);
    runtime.send_sennet(0, header(), "stale").unwrap();
    let transmission = runtime.begin_tx(0).unwrap().expect("queued transmission");
    let mut stale = transmission.id;
    stale.sequence += 1;
    assert!(matches!(
        runtime.complete_tx(1, stale, true),
        Err(Error::Work(_))
    ));
    assert!(runtime.complete_tx(1, transmission.id, true).is_ok());
}

#[test]
fn malformed_input_preserves_before_deadline_and_reports_loss_once_at_deadline() {
    let mut runtime = runtime(2, 20);
    activate_sennet(&mut runtime, 0);
    runtime.send_sennet(0, header(), "retain").unwrap();
    let pending = runtime.sennet().pending_identity();

    assert!(runtime.ingest(9, &[0]).is_err());
    assert_eq!(runtime.sennet().pending_identity(), pending);

    // Observed monotonic time remains authoritative even when the frame is
    // malformed. The loss remains reportable after the refusal, exactly once.
    assert!(runtime.ingest(10, &[0]).is_err());
    assert_eq!(runtime.sennet().pending_identity(), None);
    let report = runtime.take_report();
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::WorkLost(_)))
    );
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::Sennet(sennet::instance::InstanceEvent::PendingLost { .. })
    )));
    assert!(runtime.take_report().events.is_empty());
}

#[test]
fn sennet_dedup_and_packet_ids_survive_a_real_return_home() {
    let mut runtime = runtime(2, 20);
    activate_sennet(&mut runtime, 0);
    runtime.send_sennet(0, header(), "retain-dedup").unwrap();
    let transmission = runtime.begin_tx(0).unwrap().expect("Sennet frame");
    let frame = transmission.frame.clone();
    runtime.complete_tx(1, transmission.id, true).unwrap();
    assert_eq!(runtime.sennet().packet_id_state().next_packet_id(), 2);

    let return_step = runtime.tick(20, || [0; 16]).unwrap();
    let return_transition = return_step.transition.expect("return transition");
    runtime
        .acknowledge(20, return_transition.id, Acknowledgement::Completed)
        .unwrap();
    activate_sennet(&mut runtime, 20);

    let first = runtime.ingest(20, &frame).unwrap();
    assert!(first.events.iter().any(|event| matches!(
        event,
        Event::SennetReceived(sennet::instance::ReceiveOutcome::Text(_))
    )));
    let duplicate = runtime.ingest(21, &frame).unwrap();
    assert!(duplicate.events.iter().any(|event| matches!(
        event,
        Event::SennetReceived(sennet::instance::ReceiveOutcome::Duplicate { .. })
    )));
    runtime.send_sennet(21, header(), "next").unwrap();
    assert_eq!(runtime.sennet().packet_id_state().next_packet_id(), 3);
}

#[test]
fn delayed_tucket_ack_cancels_queued_retry_and_retains_contact() {
    let mut runtime = runtime(2, 20);
    activate(&mut runtime, 0, TUCKET);
    let mut peer = TucketNode::new(LocalIdentity::from_seed([9; 32]), false);
    let peer_hash = peer.my_hash();
    let advert = peer.advert_frame(1, b"peer");
    runtime.ingest(0, &advert).unwrap();
    assert!(runtime.tucket().node().contact(peer_hash).is_some());
    runtime.advertise_tucket(0, 1, b"resident").unwrap();
    let introduction = runtime.begin_tx(0).unwrap().expect("resident advert");
    peer.on_frame(&introduction.frame);
    runtime.complete_tx(1, introduction.id, true).unwrap();
    let operation = runtime
        .send_tucket(
            1,
            peer_hash,
            "delayed",
            tucket::node::TextRetryPolicy::default(),
            tucket::instance::SendTiming {
                timestamp: 1,
                expires_at: 100,
                allowed_until: 99,
            },
        )
        .unwrap();
    runtime.poll(1, None).unwrap();
    let first = runtime.begin_tx(1).unwrap().expect("first retry");
    let received = peer.on_frame(&first.frame);
    let ack = match received.0.as_slice() {
        [tucket::node::Event::Message { ack, .. }] => *ack,
        _ => panic!("peer accepted text"),
    };
    runtime.complete_tx(2, first.id, true).unwrap();
    runtime.poll(11, None).unwrap();
    let ack_frame = peer.ack_frame(ack);
    let report = runtime.ingest(12, &ack_frame).unwrap();
    assert!(
        report
            .events
            .iter()
            .any(|event| matches!(event, Event::TucketAcknowledged(id) if *id == operation))
    );
    assert!(runtime.begin_tx(12).unwrap().is_none());
    assert!(runtime.tucket().node().contact(peer_hash).is_some());
}

#[test]
fn inflight_custody_defers_allow_loss_and_unknown_transition_ack_recovers() {
    let mut runtime = runtime(2, 20);
    let transition = runtime
        .request(
            0,
            Excursion {
                target: SENNET,
                duration_ms: 20,
                interruption: InterruptionPolicy::AllowSessionLoss,
            },
            || [0; 16],
        )
        .unwrap()
        .transition
        .unwrap();
    runtime
        .acknowledge(0, transition.id, Acknowledgement::Completed)
        .unwrap();
    runtime.send_sennet(0, header(), "custody").unwrap();
    let tx = runtime.begin_tx(0).unwrap().expect("in flight");
    // Simulate an owner that has not yet settled hardware custody at the return
    // deadline. Even explicit loss policy must wait for the actual completion.
    let deferred = runtime.tick(20, || [0; 16]).unwrap();
    assert!(
        deferred.transition.is_none(),
        "in-flight custody cannot be discarded"
    );
    runtime.complete_tx(21, tx.id, true).unwrap();
    let step = runtime.tick(22, || [0; 16]).unwrap();
    let transition = step.transition.expect("transition after TX settles");
    assert!(matches!(
        runtime.acknowledge(22, transition.id, Acknowledgement::Unknown),
        Err(Error::RecoveryRequired)
    ));
}

fn sent(actions: retinue::node::Actions<4>) -> retinue::Packet {
    actions
        .into_iter()
        .find_map(|a| match a {
            retinue::node::Action::Send { packet, .. } => Some(packet),
            _ => None,
        })
        .unwrap()
}

fn linked_runtime() -> (
    Runtime,
    Node<8, 4, 1, 4>,
    retinue::hash::AddressHash,
    retinue::Packet,
) {
    let mut runtime = runtime(2, 20);
    let mut peer = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[9; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let blob = retinue::announce::AnnounceBlob::mint([1; 5], 1).unwrap();
    runtime.poll(0, Some(&blob)).unwrap();
    let tx = runtime.begin_tx(0).unwrap().unwrap();
    peer.ingest(0, &retinue::Packet::decode(&tx.frame).unwrap(), 0);
    runtime.complete_tx(1, tx.id, true).unwrap();
    let announce = peer.announce(&blob, None);
    runtime.ingest(1, &announce.encode()).unwrap();
    let request = sent(
        peer.open_link(runtime.retinue().node().destination(), 0, &[8; 64])
            .unwrap(),
    );
    runtime.ingest(2, &request.encode()).unwrap();
    let proof = runtime.begin_tx(2).unwrap().unwrap();
    let linked = peer.ingest(0, &retinue::Packet::decode(&proof.frame).unwrap(), 3);
    runtime.complete_tx(3, proof.id, true).unwrap();
    let id = linked
        .into_iter()
        .find_map(|a| match a {
            retinue::node::Action::LinkUp { link_id } => Some(link_id),
            _ => None,
        })
        .unwrap();
    assert!(runtime.retinue().node().has_link(id));
    (runtime, peer, id, announce)
}

#[test]
fn retinue_link_and_freshness_survive_an_admitted_home_absence() {
    let (mut runtime, peer, id, announce) = linked_runtime();
    let destination = runtime.retinue().node().destination();
    let transition = runtime
        .request(
            4,
            Excursion {
                target: SENNET,
                duration_ms: 20,
                interruption: InterruptionPolicy::ResumableOnly,
            },
            || [0; 16],
        )
        .unwrap()
        .transition
        .unwrap();
    runtime
        .acknowledge(5, transition.id, Acknowledgement::Completed)
        .unwrap();
    let back = runtime.tick(25, || [0; 16]).unwrap().transition.unwrap();
    runtime
        .acknowledge(26, back.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(runtime.retinue().node().destination(), destination);
    assert!(runtime.retinue().node().has_link(id));
    assert!(runtime.retinue().node().peers().knows(peer.destination()));
    let prior = runtime
        .retinue()
        .node()
        .transport_counters()
        .replayed_announces;
    runtime.ingest(27, &announce.encode()).unwrap();
    assert_eq!(
        runtime
            .retinue()
            .node()
            .transport_counters()
            .replayed_announces,
        prior + 1
    );
}

#[test]
fn forced_retinue_resource_loss_reports_link_resource_and_unsent_close() {
    use radio_hand::instances::Event;
    let (mut runtime, mut peer, id, _) = linked_runtime();
    let advert = sent(
        peer.publish(id, 0, &[5; 1024], [4; 4], &[6; 16], 4)
            .unwrap(),
    );
    runtime.ingest(4, &advert.encode()).unwrap();
    assert!(runtime.retinue().node().transfer_active(id));
    let refused = runtime.request(
        5,
        Excursion {
            target: SENNET,
            duration_ms: 20,
            interruption: InterruptionPolicy::ResumableOnly,
        },
        || [0; 16],
    );
    assert!(refused.is_err());
    assert!(runtime.retinue().node().has_link(id));
    let step = runtime
        .request(
            5,
            Excursion {
                target: SENNET,
                duration_ms: 20,
                interruption: InterruptionPolicy::AllowSessionLoss,
            },
            || [7; 16],
        )
        .unwrap();
    assert!(step.transition.is_some());
    let report = step
        .report
        .events
        .iter()
        .find_map(|e| match e {
            Event::RetinueInterrupted(r) => Some(r),
            _ => None,
        })
        .unwrap();
    assert_eq!(report.closed_links.as_slice(), &[id]);
    assert_eq!(report.inbound_resources.as_slice(), &[id]);
    assert_eq!(report.close_packets.len(), 1);
    assert!(
        step.report
            .events
            .iter()
            .any(|e| matches!(e, Event::WorkLost(_)))
    );
    assert!(!runtime.retinue().node().has_link(id));
}

#[test]
fn bounded_usb_report_keeps_complete_events_and_accounts_for_omissions() {
    use radio_hand::instances::{Event, Report};
    let mut report = Report::default();
    report
        .events
        .push(Event::Retinue(retinue::node::Action::Resource {
            link_id: retinue::hash::AddressHash::from_bytes([1; 16]),
            data: vec![255; 2048],
        }))
        .unwrap();
    report.events.push(Event::ActionsOverflowed(2)).unwrap();
    let text = report.format(u64::MAX);
    assert!(!text.contains("Resource"));
    assert!(text.contains("ActionsOverflowed(2)\n"));
    assert!(text.ends_with("events=2 dropped=0 omitted=1\n"));
}
