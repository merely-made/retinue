//! Interval edges: what replay will and will not certify as a complete duration.

use super::*;

#[test]
fn owner_uncertainty_does_not_invent_a_listening_stop_time() {
    let mut capture = bundle();
    admit(&mut capture, event(1, 10, listening()), 100);
    admit(
        &mut capture,
        event(2, 20, ObservationKind::ContinuityLost { cause: 3 }),
        110,
    );
    admit(&mut capture, event(3, 30, listening()), 120);
    admit(&mut capture, event(4, 40, stopped()), 130);
    let timeline = replay(&capture).unwrap();
    assert_eq!(timeline.intervals.len(), 2);
    assert_eq!(timeline.intervals[0].end_ms, None);
    assert_eq!(
        timeline.intervals[0].incomplete_reason,
        Some(IncompleteReason::OwnerUncertain)
    );
    assert_eq!(timeline.summary.listening_ms.get(&2), Some(&10));
    assert_eq!(timeline.summary.incomplete_intervals, 1);
}

#[test]
fn fresh_boot_cannot_finish_the_previous_boots_interval() {
    let mut capture = bundle();
    let mut before_reset = ObservationRecorder::<2>::new(9).unwrap();
    before_reset.record(500, listening()).unwrap();
    let mut after_reset = ObservationRecorder::<2>::new(10).unwrap();
    after_reset.record(2, stopped()).unwrap();
    for (recorder, boot_id, received) in [(&before_reset, 9, 1000), (&after_reset, 10, 900)] {
        // Host civil time moved backwards. Device uptime and boot remain separate.
        for record in recorder
            .drain(CursorRequest {
                boot_id,
                after_sequence: 0,
                max_records: 2,
            })
            .unwrap()
        {
            admit(&mut capture, record, received);
        }
    }
    let timeline = replay(&capture).unwrap();
    assert!(
        timeline
            .intervals
            .iter()
            .all(|interval| interval.edge == Edge::Incomplete)
    );
    assert!(timeline.summary.listening_ms.is_empty());
}

#[test]
fn gaps_disconnects_and_sequence_skips_never_complete_an_old_start() {
    for mode in 0..3 {
        let mut capture = bundle();
        admit(&mut capture, event(1, 100, listening()), 1000);
        match mode {
            0 => admit(
                &mut capture,
                ObservationRecord::Gap(radio_hand::observation::ObservationGap {
                    boot_id: 9,
                    first_missing: 2,
                    count: 1,
                }),
                1001,
            ),
            1 => capture.disconnect(1001).unwrap(),
            _ => {}
        }
        admit(
            &mut capture,
            event(if mode == 1 { 2 } else { 3 }, 150, stopped()),
            1002,
        );
        let timeline = replay(&capture).unwrap();
        assert!(timeline.summary.listening_ms.is_empty());
        assert!(
            timeline
                .intervals
                .iter()
                .all(|interval| interval.edge == Edge::Incomplete)
        );
        assert_eq!(timeline.intervals[0].end_ms, None);
        assert_eq!(timeline.intervals[1].start_ms, None);
        assert_eq!(
            timeline.summary.missing_records,
            if mode == 1 { 0 } else { 1 }
        );
    }
}

#[test]
fn mismatched_stop_ids_and_occupancy_do_not_certify_duration() {
    use radio_hand::observation::{QuietCause, TxOutcome};
    let cases = [
        (
            listening(),
            ObservationKind::ListeningStopped {
                assignment: 8,
                reason: StopReason::Completed,
            },
        ),
        (
            ObservationKind::TxStarted {
                profile: 2,
                work: 7,
                length: 2,
            },
            ObservationKind::TxFinished {
                work: 8,
                outcome: TxOutcome::Sent,
            },
        ),
        (
            ObservationKind::QuietStarted {
                cause: QuietCause::Configuration,
            },
            ObservationKind::QuietStopped {
                cause: QuietCause::Settings,
            },
        ),
        (ObservationKind::SleepStarted, stopped()),
    ];
    for (start, stop) in cases {
        let mut capture = bundle();
        admit(&mut capture, event(1, 100, start), 1000);
        admit(&mut capture, event(2, 150, stop), 1001);
        let timeline = replay(&capture).unwrap();
        assert_eq!(timeline.summary.incomplete_intervals, 2);
        assert!(
            timeline
                .intervals
                .iter()
                .all(|interval| interval.edge == Edge::Incomplete)
        );
    }
}

#[test]
fn a_new_occupancy_breaks_listening_but_can_form_its_own_complete_interval() {
    use radio_hand::observation::TxOutcome;
    let mut capture = bundle();
    for (sequence, time, kind) in [
        (1, 100, listening()),
        (
            2,
            110,
            ObservationKind::TxStarted {
                profile: 2,
                length: 10,
                work: 7,
            },
        ),
        (
            3,
            130,
            ObservationKind::TxFinished {
                work: 7,
                outcome: TxOutcome::Sent,
            },
        ),
        (4, 150, stopped()),
    ] {
        admit(&mut capture, event(sequence, time, kind), 1000);
    }
    let timeline = replay(&capture).unwrap();
    assert!(timeline.summary.listening_ms.is_empty());
    assert_eq!(timeline.summary.transmit_ms, 20);
    assert_eq!(timeline.intervals[0].end_ms, Some(110));
    assert_eq!(timeline.intervals[0].edge, Edge::Incomplete);
}

#[test]
fn a_capture_during_quiet_breaks_the_quiet_interval() {
    use radio_hand::observation::QuietCause;
    let mut capture = bundle();
    for (sequence, time, kind) in [
        (
            1,
            100,
            ObservationKind::QuietStarted {
                cause: QuietCause::Configuration,
            },
        ),
        (2, 110, ObservationKind::RxDamaged { profile: 2 }),
        (
            3,
            120,
            ObservationKind::QuietStopped {
                cause: QuietCause::Configuration,
            },
        ),
    ] {
        admit(&mut capture, event(sequence, time, kind), 1000);
    }
    let timeline = replay(&capture).unwrap();
    assert_eq!(timeline.summary.quiet_ms, 0);
    assert_eq!(timeline.summary.damaged, 1);
    assert_eq!(
        timeline.intervals[0].incomplete_reason,
        Some(IncompleteReason::ContradictoryCapture)
    );
}

#[test]
fn complete_occupancy_and_point_summaries_keep_causes_and_counts() {
    use radio_hand::observation::{QuietCause, RefusalReason, RequestKind, WakeCause};
    let mut capture = bundle();
    for (sequence, time, kind) in [
        (
            1,
            100,
            ObservationKind::QuietStarted {
                cause: QuietCause::Unknown(200),
            },
        ),
        (
            2,
            130,
            ObservationKind::QuietStopped {
                cause: QuietCause::Unknown(200),
            },
        ),
        (3, 200, ObservationKind::SleepStarted),
        (
            4,
            205,
            ObservationKind::SleepStopped {
                cause: WakeCause::Timer,
            },
        ),
        (
            5,
            210,
            ObservationKind::RxCaptured {
                profile: 2,
                length: 10,
                rssi_dbm: -80,
                snr_tenths_db: -10,
                capture_tag: 1,
            },
        ),
        (
            6,
            215,
            ObservationKind::WorkRefused {
                request: RequestKind::Transmit,
                reason: RefusalReason::DutyBudget,
                work: 2,
            },
        ),
    ] {
        admit(&mut capture, event(sequence, time, kind), 1000);
    }
    let timeline = replay(&capture).unwrap();
    assert_eq!(timeline.summary.quiet_ms, 30);
    assert_eq!(
        timeline.summary.quiet_by_cause_ms,
        [(QuietCause::Unknown(200), 30)]
    );
    assert_eq!(timeline.summary.sleep_ms, 5);
    assert_eq!(timeline.summary.captures, 1);
    assert_eq!(timeline.summary.refusals, 1);
    assert!(
        timeline.summary.listening_ms.is_empty(),
        "capture alone is not an interval"
    );
}

#[test]
fn repeated_listen_start_marks_only_the_later_matched_interval_complete() {
    let mut capture = bundle();
    for (sequence, time, kind) in [
        (1, 100, listening()),
        (2, 110, listening()),
        (3, 140, stopped()),
    ] {
        admit(&mut capture, event(sequence, time, kind), 1000);
    }
    let timeline = replay(&capture).unwrap();
    assert_eq!(
        timeline.intervals[0].incomplete_reason,
        Some(IncompleteReason::RepeatedStart)
    );
    assert_eq!(timeline.intervals[1].edge, Edge::Complete);
    assert_eq!(timeline.summary.listening_ms[&2], 30);
}
