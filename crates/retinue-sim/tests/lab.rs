//! The lab topology's two traces: determinism, the routes they take, and the face mapping.

#[path = "../examples/lab/scenarios.rs"]
mod scenarios;

use retinue::node::{DEFAULT_ROUTE_TTL, link_request_timeout};
use retinue_sim::trace::{Event, FaceEventKind, Origin, PacketKind, Refusal};
use retinue_sim::{NodeState, SCHEMA, Send, Trace, run};

fn path(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// Fire's route to garage after each of fire's events, as (time, via).
fn fire_route_to_garage(trace: &Trace) -> Vec<(u64, Option<String>)> {
    let mut changes: Vec<(u64, Option<String>)> = Vec::new();
    for event in &trace.events {
        let (t, state) = match event {
            Event::Transmit { t, node, state, .. } | Event::Receive { t, node, state, .. }
                if node == "fire" =>
            {
                (*t, state)
            }
            _ => continue,
        };
        let via = state
            .routes
            .iter()
            .find(|route| route.to == "garage")
            .and_then(|route| route.via.clone());
        if changes.last().map(|(_, last)| last) != Some(&via) {
            changes.push((t, via));
        }
    }
    changes
}

#[test]
fn both_traces_are_byte_identical_across_runs() {
    for scenario in [scenarios::cold(), scenarios::warm()] {
        let first = run(&scenario).unwrap().to_json();
        let second = run(&scenario).unwrap().to_json();
        assert_eq!(first.as_bytes(), second.as_bytes(), "{}", scenario.name);
        let trace = Trace::from_json(&first).unwrap();
        assert_eq!(trace.schema, SCHEMA);
        assert_eq!(trace.to_json(), first, "the schema round-trips");
    }
}

#[test]
fn cold_cut_routes_through_church_and_water() {
    let trace = run(&scenarios::cold()).unwrap();
    let send = trace
        .events
        .iter()
        .find_map(|event| match event {
            Event::Send { via, hops, .. } => Some((via.clone(), *hops)),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        send,
        (Some("church".into()), Some(2)),
        "addressed to church"
    );

    let delivery = trace.messages[0].delivered.as_ref().expect("delivered");
    assert_eq!(delivery.path, path(&["fire", "church", "water", "garage"]));
    let relays = delivery.path.len() - 2;
    let transmissions = delivery.path.len() - 1;
    assert_eq!((relays, transmissions), (2, 3));

    let shortcut_used = trace.events.iter().any(|event| match event {
        Event::Transmit { node, heard_by, .. } => {
            (node == "fire" && heard_by.contains(&"water".to_owned()))
                || (node == "water" && heard_by.contains(&"fire".to_owned()))
        }
        _ => false,
    });
    assert!(!shortcut_used, "nothing crosses the cut edge");
}

/// Fire is a leaf: in both traces it relays nothing, yet every send names a first relay.
#[test]
fn the_leaf_sender_forwards_nothing_and_still_addresses_its_relay() {
    for scenario in [scenarios::cold(), scenarios::warm()] {
        let trace = run(&scenario).unwrap();
        let fire = trace.nodes.iter().find(|node| node.name == "fire").unwrap();
        assert!(!fire.transit);
        assert!(!trace.events.iter().any(|event| matches!(
            event,
            Event::Transmit { node, origin: Origin::Forward, .. } if node == "fire"
        )));
        assert!(
            trace
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::Send { via, .. } => Some(via),
                    _ => None,
                })
                .all(Option::is_some),
            "{}",
            scenario.name
        );
    }
}

#[test]
fn warm_cut_loses_sends_until_the_next_announce_then_reroutes() {
    let scenario = scenarios::warm();
    let cut_at = scenario.cuts[0].at;
    let trace = run(&scenario).unwrap();
    let [first, lost @ .., rerouted] = trace.messages.as_slice() else {
        panic!("five messages");
    };

    let first = first
        .delivered
        .as_ref()
        .expect("the first send crosses the shortcut");
    assert!(first.t < cut_at);
    assert_eq!(first.path, path(&["fire", "water", "garage"]));

    assert_eq!(lost.len(), 3);
    for message in lost {
        assert!(
            message.delivered.is_none(),
            "message {} is lost",
            message.id
        );
        // Its request names water, which cannot hear it; church hears it and drops it.
        let request = trace
            .events
            .iter()
            .find_map(|event| match event {
                Event::Transmit {
                    t,
                    node,
                    origin: Origin::App,
                    packet,
                    heard_by,
                    blocked,
                    ..
                } if *t == message.sent_at
                    && node == "fire"
                    && packet.packet_type == PacketKind::LinkRequest =>
                {
                    Some((packet.transport.clone(), heard_by.clone(), blocked.clone()))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            request,
            (
                Some("water".into()),
                vec!["church".into()],
                vec!["water".into()]
            )
        );
    }

    let rerouted = rerouted.delivered.as_ref().expect("the reroute delivers");
    assert_eq!(rerouted.path, path(&["fire", "church", "water", "garage"]));

    // Fire's route moves from water to church once garage's next announce arrives: at the
    // announce interval, long before the route TTL would have expired it.
    let changes = fire_route_to_garage(&trace);
    let [.., (via_water_at, water), (via_church_at, church)] = changes.as_slice() else {
        panic!("route history {changes:?}");
    };
    assert_eq!(water.as_deref(), Some("water"));
    assert_eq!(church.as_deref(), Some("church"));
    let interval = scenario.timing.announce_interval;
    assert!(*via_church_at >= interval && *via_church_at < interval + 1_000);
    assert!(*via_church_at < via_water_at + DEFAULT_ROUTE_TTL);
    for message in lost {
        assert!(message.sent_at > cut_at && message.sent_at <= *via_church_at);
    }
}

/// Every node state fills radio-face's TRAFFIC page and ticker without truncation.
#[test]
fn node_states_fill_the_traffic_page_and_ticker() {
    let mut checked = 0;
    for scenario in [scenarios::cold(), scenarios::warm()] {
        for event in run(&scenario).unwrap().events {
            let state: NodeState = match event {
                Event::Transmit { state, .. }
                | Event::Receive { state, .. }
                | Event::LinkRequestExpired { state, .. } => state,
                _ => continue,
            };
            let local = radio_face::LocalStatus {
                tx_frames: state.tx_frames,
                rx_frames: state.rx_frames,
                last_tx: state
                    .last_tx_len
                    .map_or(radio_face::TxResult::None, |frame_len| {
                        radio_face::TxResult::Sent { frame_len }
                    }),
                last_rx: state.last_rx_len.map(|frame_len| radio_face::RxSummary {
                    frame_len,
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert_eq!(local.tx_frames, state.tx_frames);
            let event = state.event.map(|event| radio_face::UiEvent {
                source: radio_face::EventSource::Local,
                kind: match event.kind {
                    FaceEventKind::Info => radio_face::EventKind::Info,
                    FaceEventKind::Received => radio_face::EventKind::Received,
                    FaceEventKind::Transmitted => radio_face::EventKind::Transmitted,
                    FaceEventKind::Delivered => radio_face::EventKind::Delivered,
                    FaceEventKind::Propagated => radio_face::EventKind::Propagated,
                    FaceEventKind::Failed => radio_face::EventKind::Failed,
                },
                text: radio_face::Text::try_from_str(&event.text).expect("fits the ticker"),
            });
            let host = radio_face::HostSnapshot {
                personality: radio_face::Personality::Retinue,
                link_count: state.links as u8,
                admitted_links: state.links as u8,
                event,
                ..Default::default()
            };
            let mut wire = [0_u8; radio_face::MAX_SNAPSHOT_LEN];
            radio_face::encode_snapshot(&host, &mut wire).expect("a valid snapshot");
            checked += 1;
        }
    }
    assert!(checked > 100);
}

/// S7's wedge, end to end. Four sends lost behind the cut fill fire's four pending slots, and
/// a fifth inside their deadline is refused. After the deadline the slots are free, so the
/// send after the reroute goes out and is delivered, where before Ruling 28 it was refused.
#[test]
fn lost_requests_expire_so_the_send_after_the_reroute_is_not_refused() {
    let mut scenario = scenarios::warm();
    let send = |at: u64, n: u32| Send {
        at,
        from: "fire".into(),
        to: "garage".into(),
        payload: format!("message {n}"),
    };
    // Fire's route to garage is via water, one relay, until the 600 s announce.
    let deadline = link_request_timeout(1);
    scenario.sends = vec![
        send(10_000, 1),
        send(180_000, 2),
        send(181_000, 3),
        send(182_000, 4),
        send(183_000, 5),
        send(184_000, 6),
        send(720_000, 7),
    ];
    assert!(184_000 < 180_000 + deadline);
    let trace = run(&scenario).unwrap();

    let refused: Vec<(u32, Refusal)> = trace
        .events
        .iter()
        .filter_map(|event| match event {
            Event::SendRefused {
                message, reason, ..
            } => Some((*message, *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(
        refused,
        vec![(5, Refusal::PendingFull)],
        "only the send inside the lost requests' deadline is refused"
    );
    for lost in &trace.messages[1..5] {
        assert!(lost.delivered.is_none(), "message {} is lost", lost.id);
    }
    let last = trace.messages[6]
        .delivered
        .as_ref()
        .expect("the send after the reroute is delivered");
    assert_eq!(last.path, path(&["fire", "church", "water", "garage"]));
}

/// The trace's `link_request_expired` events, as (index, time, node, message).
fn expiries(trace: &Trace) -> Vec<(usize, u64, String, Option<u32>)> {
    trace
        .events
        .iter()
        .enumerate()
        .filter_map(|(i, event)| match event {
            Event::LinkRequestExpired {
                t, node, message, ..
            } => Some((i, *t, node.clone(), *message)),
            _ => None,
        })
        .collect()
}

/// Ruling 55: each lost warm send's request expires at the first poll at or after its
/// deadline, one relay away, and the event carries fire's state with the face line.
/// Nothing expires in the cold trace, where the one send is delivered.
#[test]
fn warm_lost_requests_expire_at_their_deadlines() {
    assert!(expiries(&run(&scenarios::cold()).unwrap()).is_empty());

    let scenario = scenarios::warm();
    let trace = run(&scenario).unwrap();
    let poll = scenario.timing.poll_interval;
    let expected: Vec<(u64, String, Option<u32>)> = trace.messages[1..4]
        .iter()
        .map(|lost| {
            let deadline = lost.sent_at + link_request_timeout(1);
            (
                deadline.div_ceil(poll) * poll,
                "fire".to_owned(),
                Some(lost.id),
            )
        })
        .collect();
    let got: Vec<_> = expiries(&trace)
        .into_iter()
        .map(|(_, t, node, message)| (t, node, message))
        .collect();
    assert_eq!(got, expected);
    assert_eq!(
        got.iter().map(|(t, ..)| *t).collect::<Vec<_>>(),
        [200_000, 380_000, 560_000]
    );
    for event in &trace.events {
        if let Event::LinkRequestExpired {
            link,
            state,
            message,
            ..
        } = event
        {
            assert_eq!(
                Some(link),
                trace.messages[message.unwrap() as usize].link.as_ref()
            );
            assert_eq!(state.pending_links, 0);
            let face = state.event.as_ref().unwrap();
            assert_eq!(
                (face.kind, face.text.as_str()),
                (FaceEventKind::Failed, "link unanswered")
            );
        }
    }
}

/// Ruling 54 through the trace: a send at a deadline, before that tick's poll, expires the
/// overdue requests inside `open_link`. They are recorded ahead of the send that freed their
/// slots; one not yet due waits for a later poll.
#[test]
fn open_link_expiries_precede_the_send_that_freed_them() {
    let mut scenario = scenarios::warm();
    let send = |at: u64, n: u32| Send {
        at,
        from: "fire".into(),
        to: "garage".into(),
        payload: format!("message {n}"),
    };
    let deadline = link_request_timeout(1);
    scenario.sends = vec![
        send(10_000, 1),
        send(180_000, 2),
        send(181_000, 3),
        send(182_000, 4),
        send(183_000, 5),
        send(180_000 + 20_000, 6),
    ];
    // Three deadlines fall at or before 200 s; the fourth, 201 s, does not.
    assert_eq!(182_000 + deadline, 200_000);
    let trace = run(&scenario).unwrap();

    let send_6 = trace
        .events
        .iter()
        .position(|event| matches!(event, Event::Send { message: 5, .. }))
        .expect("the sixth send goes out");
    let got = expiries(&trace);
    let at_send: Vec<_> = got.iter().filter(|(_, t, ..)| *t == 200_000).collect();
    assert_eq!(
        at_send.iter().map(|(.., m)| *m).collect::<Vec<_>>(),
        [Some(1), Some(2), Some(3)]
    );
    assert!(at_send.iter().all(|(i, ..)| *i < send_6));
    assert!(
        !trace
            .events
            .iter()
            .any(|event| matches!(event, Event::SendRefused { .. }))
    );
    // The fourth expires at the next poll after its own deadline, and the sixth send, lost
    // behind the cut like the others, at the poll after its own.
    assert_eq!(
        got.iter()
            .filter(|(_, t, ..)| *t != 200_000)
            .map(|(_, t, _, m)| (*t, *m))
            .collect::<Vec<_>>(),
        [(205_000, Some(4)), (220_000, Some(5))]
    );
}
