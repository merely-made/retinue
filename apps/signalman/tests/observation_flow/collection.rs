//! Collection, admission, and replay of captured records.

use super::*;

#[test]
fn paginated_overwrite_gap_prevents_false_listening_coverage() {
    let mut recorder = ObservationRecorder::<2>::new(9).unwrap();
    let mut capture = bundle();
    recorder.record(100, listening()).unwrap();
    let request = |after_sequence| CursorRequest {
        boot_id: 9,
        after_sequence,
        max_records: 1,
    };
    admit(
        &mut capture,
        recorder.drain(request(0)).unwrap().next().unwrap(),
        1000,
    );
    recorder
        .record(110, ObservationKind::RxDamaged { profile: 2 })
        .unwrap();
    recorder.record(120, stopped()).unwrap();
    recorder.record(200, listening()).unwrap();
    recorder.record(230, stopped()).unwrap();

    let before = recorder.stats();
    let mut cursor = 1;
    let mut cursors = Vec::new();
    loop {
        let mut page = recorder.drain(request(cursor)).unwrap();
        let Some(record) = page.next() else { break };
        assert_eq!(page.next(), None);
        cursor = page.next_cursor();
        cursors.push(cursor);
        admit(&mut capture, record, 1000 + cursor);
    }
    assert_eq!(cursors, [3, 4, 5]);
    assert_eq!(recorder.stats(), before, "collection must be read-only");
    let timeline = replay(&capture).unwrap();
    assert_eq!(timeline.intervals.len(), 2);
    assert_eq!(timeline.intervals[0].edge, Edge::Incomplete);
    let complete = &timeline.intervals[1];
    assert_eq!(complete.edge, Edge::Complete);
    assert_eq!((complete.start_ms, complete.end_ms), (Some(200), Some(230)));
    assert_eq!(timeline.gaps.len(), 1);
    assert_eq!(timeline.summary.missing_records, 2);
    assert_eq!(timeline.summary.listening_ms.get(&2), Some(&30));
}

#[test]
fn independent_mixed_boot_literals_replay_without_reencoding() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/observation_mixed_boot.json")).unwrap();
    let mut capture = bundle();
    for row in fixture["records"].as_array().unwrap() {
        let raw = hex::decode(row["hex"].as_str().unwrap()).unwrap();
        let ObservationRecord::Event(decoded) = ObservationRecord::decode(&raw).unwrap() else {
            panic!("event expected")
        };
        assert_eq!(decoded.boot_id, row["boot"].as_u64().unwrap());
        assert_eq!(decoded.sequence, row["sequence"].as_u64().unwrap());
        assert_eq!(decoded.uptime_ms, row["uptime_ms"].as_u64().unwrap());
        capture.admit(&raw, 1_000_000).unwrap();
    }
    let timeline = replay(&capture).unwrap();
    assert_eq!(
        timeline.summary.listening_ms[&2],
        fixture["expected"]["complete_listening_ms"]
            .as_u64()
            .unwrap() as u128
    );
    assert_eq!(timeline.summary.sleep_ms, 5);
    assert_eq!(timeline.intervals.len(), 3);
    assert_eq!(
        timeline.intervals[1].incomplete_reason,
        Some(IncompleteReason::UnknownEvent)
    );
    assert_eq!(
        (timeline.intervals[1].start_ms, timeline.intervals[1].end_ms),
        (Some(200), Some(210))
    );
    assert!(matches!(
        timeline.events[3].kind,
        ObservationKind::Unknown {
            kind: 200,
            len: 0,
            ..
        }
    ));
    assert_eq!(capture.carrier(), CarrierKind::LocalUsb);
    assert_eq!(capture.device(), b"bench-board");
    assert_eq!(replay(&capture).unwrap(), timeline);
}

#[test]
fn unchanged_cursor_replays_are_idempotent_but_conflicts_and_backwards_events_fail() {
    let mut capture = bundle();
    let start = event(1, 100, listening());
    let stop = event(2, 130, stopped());
    for record in [start, stop, start, stop] {
        admit(&mut capture, record, 1000);
    }
    let timeline = replay(&capture).unwrap();
    assert_eq!(timeline.duplicates, 2);
    assert_eq!(timeline.summary.listening_ms[&2], 30);
    admit(&mut capture, event(2, 131, stopped()), 1000);
    assert_eq!(replay(&capture), Err(ReplayError::ConflictingSequence));

    let mut backwards = bundle();
    admit(&mut backwards, event(2, 100, listening()), 1000);
    admit(&mut backwards, event(1, 110, stopped()), 1000);
    assert_eq!(replay(&backwards), Err(ReplayError::NonMonotonicSequence));
    let mut clock = bundle();
    admit(&mut clock, event(1, 100, listening()), 1000);
    admit(&mut clock, event(2, 90, stopped()), 1100);
    assert_eq!(replay(&clock), Err(ReplayError::NonMonotonicTime));
}

#[test]
fn overlapping_cursor_loss_counts_only_still_missing_events() {
    let mut capture = bundle();
    admit(&mut capture, event(1, 100, listening()), 1000);
    admit(&mut capture, event(2, 110, stopped()), 1000);
    let gap = ObservationRecord::Gap(radio_hand::observation::ObservationGap {
        boot_id: 9,
        first_missing: 1,
        count: 4,
    });
    admit(&mut capture, gap, 1000);
    admit(&mut capture, gap, 1001);
    let timeline = replay(&capture).unwrap();
    assert_eq!(timeline.summary.listening_ms[&2], 10);
    assert_eq!(timeline.summary.missing_records, 2);
    assert_eq!(timeline.gaps[0].gap.first_missing, 3);
    assert_eq!(timeline.duplicates, 1);
}

#[test]
fn full_sequence_range_is_explicit_loss_without_overflow() {
    let mut capture = bundle();
    admit(
        &mut capture,
        ObservationRecord::Gap(radio_hand::observation::ObservationGap {
            boot_id: 9,
            first_missing: 1,
            count: u64::MAX,
        }),
        1000,
    );
    assert_eq!(
        replay(&capture).unwrap().summary.missing_records,
        u128::from(u64::MAX)
    );
}

#[test]
fn admission_accounts_for_metadata_and_host_edges_without_mutating_on_failure() {
    let profile = profile();
    let mut limited = ObservationBundle::new(
        BUNDLE_VERSION,
        b"board",
        CarrierKind::Imported,
        "fixture",
        std::slice::from_ref(&profile),
        Admission {
            max_frames: 1,
            max_bytes: 4096,
        },
    )
    .unwrap();
    limited.disconnect(0).unwrap();
    let before = limited.clone();
    assert_eq!(limited.disconnect(1), Err(AdmissionError::TooManyFrames));
    assert_eq!(limited, before);
    assert_eq!(
        ObservationBundle::new(
            BUNDLE_VERSION,
            b"board",
            CarrierKind::Imported,
            "fixture",
            std::slice::from_ref(&profile),
            Admission {
                max_frames: 1,
                max_bytes: 1
            }
        ),
        Err(AdmissionError::TooManyBytes)
    );
    assert_eq!(
        ObservationBundle::new(
            BUNDLE_VERSION,
            b"board",
            CarrierKind::Imported,
            "fixture",
            &[profile.clone(), profile],
            Admission {
                max_frames: 1,
                max_bytes: 4096
            }
        ),
        Err(AdmissionError::DuplicateProfile(2))
    );
    let mut capture = bundle();
    let before = capture.clone();
    assert!(matches!(
        capture.admit(&[0; 65], 0),
        Err(AdmissionError::InvalidRecord(_))
    ));
    assert!(matches!(
        capture.admit(&[0; 34], 0),
        Err(AdmissionError::InvalidRecord(_))
    ));
    assert_eq!(capture, before);
    let mut raw = [0; MAX_RECORD_BYTES];
    let len = event(1, 1, ObservationKind::RxDamaged { profile: 3 })
        .encode(&mut raw)
        .unwrap();
    assert_eq!(
        capture.admit(&raw[..len], 0),
        Err(AdmissionError::UnknownProfile(3))
    );
}

#[test]
fn source_and_host_time_and_future_reasons_remain_inspectable() {
    let mut capture = bundle();
    admit(&mut capture, event(1, 100, listening()), 1000);
    admit(
        &mut capture,
        event(
            2,
            110,
            ObservationKind::ListeningStopped {
                assignment: 7,
                reason: StopReason::Unknown(201),
            },
        ),
        900,
    );
    let timeline = replay(&capture).unwrap();
    assert_eq!(timeline.summary.listening_ms[&2], 10);
    assert!(matches!(
        timeline.events[1].kind,
        ObservationKind::ListeningStopped {
            reason: StopReason::Unknown(201),
            ..
        }
    ));
    let BundleEntry::Record(frame) = &capture.entries()[1] else {
        panic!("record expected")
    };
    assert_eq!(frame.received_unix_ms, 900);
    assert_eq!(timeline.events[1].uptime_ms, 110);
    assert!(matches!(
        timeline.intervals[0].activity,
        Activity::Listening {
            assignment: 7,
            profile: Some(2)
        }
    ));
}
