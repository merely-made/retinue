//! The lab topology's two traces: determinism, the routes they take, and the face mapping.

#[path = "../examples/lab/scenarios.rs"]
mod scenarios;

use retinue::node::DEFAULT_ROUTE_TTL;
use retinue_sim::trace::{Event, FaceEventKind, Origin, PacketKind};
use retinue_sim::{NodeState, SCHEMA, Trace, run};

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
                Event::Transmit { state, .. } | Event::Receive { state, .. } => state,
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
