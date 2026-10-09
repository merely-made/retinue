//! Retinue links across home absences, resource loss, and first-hop expiry rulings.

use super::*;

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
        peer.open_link(runtime.retinue().node().destination(), 0, &[8; 64], 1)
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

/// Ruling 50 in the resident runtime: the Retinue profile's modulation sets the node's
/// first-hop airtime allowance on the instance's interface. A request already pending when
/// the runtime is built keeps its deadline (see the Ruling 46 test below).
#[test]
fn the_retinue_profile_sets_the_first_hop_allowance() {
    let mut node = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[1; 64]),
        DestinationName::new("retinue", ["resident"]).name_hash(),
    );
    let peer = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[9; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let blob = retinue::announce::AnnounceBlob::mint([1; 5], 1).unwrap();
    node.ingest(0, &peer.announce(&blob, None), 0);
    assert_eq!(node.first_hop_airtime(retinue::instance::INTERFACE), 0);
    let runtime = runtime_from(node, 2, 20, Default::default());
    // LongFast: SF11, 250 kHz, 4/5, about 1,074 bps.
    let allowance = runtime
        .retinue()
        .node()
        .first_hop_airtime(retinue::instance::INTERFACE);
    assert_eq!(allowance, 3_724);
    assert_eq!(
        Some(allowance),
        radio_hand::phy::nominal_bits_ms(11, 250_000, 5, retinue::node::FIRST_HOP_ALLOWANCE_BITS)
    );
}

/// Rulings 46 and 74 in the resident runtime: the runtime's own expiry pass reconciles the
/// node before polling it, and an unanswered request surfaces as the `LinkRequestTimedOut`
/// action the channel node reports, with no `RetinueExpired`. A pass that expired only a
/// request was once dropped from the report entirely.
#[test]
fn an_unanswered_request_is_reported_as_timed_out() {
    let mut node = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[1; 64]),
        DestinationName::new("retinue", ["resident"]).name_hash(),
    );
    let peer = Node::<8, 4, 1, 4>::new(
        PrivateIdentity::from_secret_bytes(&[9; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let blob = retinue::announce::AnnounceBlob::mint([1; 5], 1).unwrap();
    node.ingest(0, &peer.announce(&blob, None), 0);
    let request = sent(node.open_link(peer.destination(), 0, &[8; 64], 0).unwrap());
    let id = retinue::link::link_id(&request).unwrap();
    let mut runtime = runtime_from(node, 2, 20, Default::default());
    let deadline = retinue::node::link_request_timeout(0);
    // Every Retinue event in a report: a timed-out request by its id, anything else by kind.
    let expired = |report: &radio_hand::instances::Report| {
        report
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Retinue(retinue::node::Action::LinkRequestTimedOut { link_id }) => {
                    Some(("timed out", Some(*link_id)))
                }
                Event::Retinue(_) => Some(("other action", None)),
                Event::RetinueExpired(_) => Some(("expiry report", None)),
                _ => None,
            })
            .collect::<std::vec::Vec<_>>()
    };

    let before = runtime.poll(deadline - 1, None).unwrap();
    assert!(
        expired(&before).is_empty(),
        "control: nothing expires early"
    );
    assert_eq!(
        runtime
            .retinue()
            .node()
            .pause_assessment()
            .pending_handshakes,
        1
    );

    let at = runtime.poll(deadline, None).unwrap();
    assert_eq!(expired(&at), vec![("timed out", Some(id))]);
    assert_eq!(
        runtime
            .retinue()
            .node()
            .pause_assessment()
            .pending_handshakes,
        0
    );
}

/// Positive control for Ruling 74: an established link that idles out is still reported
/// in `RetinueExpired`, whose `pending_links` stays empty.
#[test]
fn an_idle_link_is_still_reported_as_expired() {
    let (mut runtime, _peer, id, _announce) = linked_runtime();
    let at = runtime
        .poll(3 + retinue::node::LINK_IDLE_TIMEOUT, None)
        .unwrap();
    let reports: std::vec::Vec<_> = at
        .events
        .iter()
        .filter_map(|event| match event {
            Event::RetinueExpired(r) => Some((r.links.to_vec(), r.pending_links.len())),
            Event::Retinue(retinue::node::Action::LinkRequestTimedOut { .. }) => {
                panic!("no request was pending")
            }
            _ => None,
        })
        .collect();
    assert_eq!(reports, vec![(vec![id], 0)]);
    assert!(!runtime.retinue().node().has_link(id));
}
