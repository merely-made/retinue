//! The face mapping and the face track, over the lab's two traces.

#[path = "../examples/lab/scenarios.rs"]
mod scenarios;

use retinue_sim::face::{FACE_SCHEMA, Face, FaceTrack, HOST_SCHEMA, LOCAL_SCHEMA, face};
use retinue_sim::trace::{Event, FaceEventKind};
use retinue_sim::{NodeState, SCHEMA, Trace, run};

fn traces() -> [Trace; 2] {
    [
        run(&scenarios::cold()).unwrap(),
        run(&scenarios::warm()).unwrap(),
    ]
}

/// Each event that carries a node state, as (index, time, node, state).
fn states(trace: &Trace) -> Vec<(usize, u64, &str, &NodeState)> {
    trace
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            Event::Transmit { t, node, state, .. }
            | Event::Receive { t, node, state, .. }
            | Event::LinkRequestExpired { t, node, state, .. } => {
                Some((index, *t, node.as_str(), state))
            }
            _ => None,
        })
        .collect()
}

/// Every node state fills radio-face's TRAFFIC page and ticker without truncation, and the
/// host snapshot passes radio-face's own wire codec.
#[test]
fn node_states_fill_the_traffic_page_and_ticker() {
    let mut checked = 0;
    let mut kinds = Vec::new();
    for trace in traces() {
        for (_, _, _, state) in states(&trace) {
            let Face { local, host } = face(state);
            assert_eq!(
                (local.tx_frames, local.rx_frames),
                (state.tx_frames, state.rx_frames)
            );
            assert_eq!(
                local.last_tx,
                state
                    .last_tx_len
                    .map_or(radio_face::TxResult::None, |frame_len| {
                        radio_face::TxResult::Sent { frame_len }
                    })
            );
            assert_eq!(
                local.last_rx.map(|rx| rx.frame_len),
                state.last_rx_len,
                "no RSSI or SNR is simulated"
            );
            assert_eq!(host.personality, radio_face::Personality::Retinue);
            assert_eq!(
                (u32::from(host.link_count), u32::from(host.admitted_links)),
                (state.links, state.links)
            );
            assert_eq!(host.queue_depth, 0);
            match (&state.event, host.event) {
                (None, None) => {}
                (Some(expected), Some(event)) => {
                    assert_eq!(event.source, radio_face::EventSource::Local);
                    assert_eq!(event.text.as_str(), expected.text, "fits the ticker");
                    kinds.push(expected.kind);
                }
                other => panic!("event mapped as {other:?}"),
            }
            let mut wire = [0_u8; radio_face::MAX_SNAPSHOT_LEN];
            radio_face::encode_snapshot(&host, &mut wire).expect("a valid snapshot");
            checked += 1;
        }
    }
    assert!(checked > 100);
    assert!(kinds.contains(&FaceEventKind::Info), "links come up");
    assert!(
        kinds.contains(&FaceEventKind::Failed),
        "warm requests go unanswered"
    );
}

#[test]
fn face_tracks_are_byte_identical_across_runs() {
    for scenario in [scenarios::cold(), scenarios::warm()] {
        let trace = run(&scenario).unwrap();
        let first = FaceTrack::from_trace(&trace).to_json();
        let second = FaceTrack::from_trace(&run(&scenario).unwrap()).to_json();
        assert_eq!(first.as_bytes(), second.as_bytes(), "{}", scenario.name);

        let track = FaceTrack::from_json(&first).unwrap();
        assert_eq!(track.to_json(), first, "the schema round-trips");
        assert_eq!(track.schema, FACE_SCHEMA);
        assert_eq!(track.route_trace, SCHEMA);
        assert_eq!(track.scenario, scenario.name);
        let digest = retinue::hash::full_hash(trace.to_json().as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            track.trace_sha256, hex,
            "names the route trace it came from"
        );
    }
}

/// Each entry's documents, serialized on their own, are what radio-mirror's
/// `set_local_json` and `set_host_json` read, and they read back as the mapped face.
#[test]
fn every_face_track_entry_is_accepted_by_radio_mirror() {
    assert_eq!(LOCAL_SCHEMA, radio_mirror::input::LOCAL_SCHEMA);
    assert_eq!(HOST_SCHEMA, radio_mirror::input::HOST_SCHEMA);
    let mut checked = 0;
    for trace in traces() {
        let track = FaceTrack::from_trace(&trace);
        let states = states(&trace);
        assert_eq!(track.entries.len(), states.len(), "{}", trace.scenario);
        for (entry, (index, t, node, state)) in track.entries.iter().zip(states) {
            assert_eq!(
                (entry.event as usize, entry.t, entry.node.as_str()),
                (index, t, node)
            );
            let expected = face(state);
            let local = serde_json::to_string(&entry.local).unwrap();
            let host = serde_json::to_string(&entry.host).unwrap();
            assert_eq!(
                radio_mirror::input::local_from_json(&local),
                Ok(expected.local),
                "{local}"
            );
            assert_eq!(
                radio_mirror::input::host_from_json(&host),
                Ok(expected.host),
                "{host}"
            );
            checked += 1;
        }
    }
    assert!(checked > 100);
}

/// A face draws the TRAFFIC page on both surfaces through radio-mirror, its face line included.
#[test]
fn a_track_entry_draws_on_the_mirror() {
    let trace = run(&scenarios::warm()).unwrap();
    let track = FaceTrack::from_trace(&trace);
    let entry = track
        .entries
        .iter()
        .rev()
        .find(|entry| entry.host.event.is_some())
        .expect("an entry with a face line");
    let local = radio_mirror::input::local_from_json(&serde_json::to_string(&entry.local).unwrap())
        .unwrap();
    let host =
        radio_mirror::input::host_from_json(&serde_json::to_string(&entry.host).unwrap()).unwrap();
    let text = entry.host.event.as_ref().unwrap().text.to_ascii_uppercase();
    for surface in [
        radio_face::Surface::Oled128x64,
        radio_face::Surface::Tft240x135,
    ] {
        let mut mirror = radio_mirror::Mirror::new(surface, radio_face::InputProfile::default());
        mirror.set_local(local);
        mirror.set_host(Some(host));
        let traffic = radio_face::Screen::Page(radio_face::Page::Traffic);
        assert!(!mirror.render_screen(traffic).is_empty());
        let lines = mirror.text_for(traffic).join("\n").to_ascii_uppercase();
        assert!(lines.contains(&text), "{surface:?}: {lines}");
    }
}
