//! Allocation-free, overwrite-on-full observation recording.
//!
//! This recorder is deliberately a radio-free primitive.  The radio owner calls
//! [`ObservationRecorder::record`] at its own transitions; diagnostic callers
//! borrow a read-only [`ObservationDrain`].  Draining does not acknowledge,
//! erase, or otherwise alter retained observations.

use super::{
    EncodeError, MAX_RECORD_BYTES, ObservationEvent, ObservationGap, ObservationKind,
    ObservationRecord,
};

/// Fixed diagnostics retained by one recorder instance.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecorderStats {
    pub recorded: u64,
    pub overwritten: u64,
    pub encode_failed: u64,
}

/// Bounds captured when a drain begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotBounds {
    pub boot_id: u64,
    pub oldest_available: Option<u64>,
    pub newest_available: Option<u64>,
}

/// A read-only request for observations after one event sequence number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorRequest {
    pub boot_id: u64,
    pub after_sequence: u64,
    pub max_records: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecorderInitError {
    InvalidBootId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordError {
    CapacityZero,
    SequenceExhausted,
    UptimeRegressed { previous_ms: u64, current_ms: u64 },
    Encode(EncodeError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorError {
    WrongBoot {
        expected: u64,
        requested: u64,
    },
    FutureSequence {
        newest_available: Option<u64>,
        requested: u64,
    },
}

/// A const-capacity, RAM-only event ring.
///
/// A full ring overwrites its oldest event.  The loss is made visible by a
/// synthetic [`ObservationGap`] on a later drain from an older cursor.
pub struct ObservationRecorder<const CAPACITY: usize> {
    boot_id: u64,
    next_sequence: Option<u64>,
    last_uptime_ms: Option<u64>,
    slots: [Option<ObservationRecord>; CAPACITY],
    oldest_index: usize,
    len: usize,
    stats: RecorderStats,
}

impl<const CAPACITY: usize> ObservationRecorder<CAPACITY> {
    pub const fn new(boot_id: u64) -> Result<Self, RecorderInitError> {
        if boot_id == 0 {
            return Err(RecorderInitError::InvalidBootId);
        }
        Ok(Self {
            boot_id,
            next_sequence: Some(1),
            last_uptime_ms: None,
            slots: [None; CAPACITY],
            oldest_index: 0,
            len: 0,
            stats: RecorderStats {
                recorded: 0,
                overwritten: 0,
                encode_failed: 0,
            },
        })
    }

    pub const fn boot_id(&self) -> u64 {
        self.boot_id
    }

    pub const fn stats(&self) -> RecorderStats {
        self.stats
    }

    pub const fn bounds(&self) -> SnapshotBounds {
        if self.len == 0 {
            return SnapshotBounds {
                boot_id: self.boot_id,
                oldest_available: None,
                newest_available: None,
            };
        }
        let oldest = match self.slots[self.oldest_index] {
            Some(ObservationRecord::Event(event)) => event.sequence,
            _ => unreachable!(),
        };
        let newest_index = (self.oldest_index + self.len - 1) % CAPACITY;
        let newest = match self.slots[newest_index] {
            Some(ObservationRecord::Event(event)) => event.sequence,
            _ => unreachable!(),
        };
        SnapshotBounds {
            boot_id: self.boot_id,
            oldest_available: Some(oldest),
            newest_available: Some(newest),
        }
    }

    /// Records a source event and returns its per-boot sequence number.
    pub fn record(&mut self, uptime_ms: u64, kind: ObservationKind) -> Result<u64, RecordError> {
        if CAPACITY == 0 {
            return Err(RecordError::CapacityZero);
        }
        if let Some(previous_ms) = self.last_uptime_ms
            && uptime_ms < previous_ms
        {
            return Err(RecordError::UptimeRegressed {
                previous_ms,
                current_ms: uptime_ms,
            });
        }
        let sequence = self.next_sequence.ok_or(RecordError::SequenceExhausted)?;
        let record = ObservationRecord::Event(ObservationEvent {
            boot_id: self.boot_id,
            sequence,
            uptime_ms,
            kind,
        });
        let mut encoded = [0_u8; MAX_RECORD_BYTES];
        if let Err(error) = record.encode(&mut encoded) {
            self.stats.encode_failed = self.stats.encode_failed.saturating_add(1);
            return Err(RecordError::Encode(error));
        }

        if self.len == CAPACITY {
            self.slots[self.oldest_index] = Some(record);
            self.oldest_index = (self.oldest_index + 1) % CAPACITY;
            self.stats.overwritten = self.stats.overwritten.saturating_add(1);
        } else {
            let index = (self.oldest_index + self.len) % CAPACITY;
            self.slots[index] = Some(record);
            self.len += 1;
        }
        self.last_uptime_ms = Some(uptime_ms);
        self.next_sequence = sequence.checked_add(1);
        self.stats.recorded = self.stats.recorded.saturating_add(1);
        Ok(sequence)
    }

    /// Borrows a bounded snapshot-style drain without mutating the ring.
    pub fn drain(
        &self,
        request: CursorRequest,
    ) -> Result<ObservationDrain<'_, CAPACITY>, CursorError> {
        if request.boot_id != self.boot_id {
            return Err(CursorError::WrongBoot {
                expected: self.boot_id,
                requested: request.boot_id,
            });
        }
        let snapshot = self.bounds();
        if request.after_sequence > snapshot.newest_available.unwrap_or(0) {
            return Err(CursorError::FutureSequence {
                newest_available: snapshot.newest_available,
                requested: request.after_sequence,
            });
        }
        let oldest = snapshot.oldest_available;
        let gap = oldest.and_then(|oldest| {
            if request.after_sequence < oldest.saturating_sub(1) {
                let first_missing = request.after_sequence + 1;
                Some(ObservationGap {
                    boot_id: self.boot_id,
                    first_missing,
                    count: oldest - first_missing,
                })
            } else {
                None
            }
        });
        let next_offset = match oldest {
            Some(oldest) if request.after_sequence >= oldest => {
                (request.after_sequence - oldest + 1) as usize
            }
            _ => 0,
        };
        Ok(ObservationDrain {
            recorder: self,
            snapshot,
            remaining: request.max_records,
            gap,
            next_offset,
            next_cursor: request.after_sequence,
        })
    }

    fn record_at_offset(&self, offset: usize) -> ObservationRecord {
        let index = (self.oldest_index + offset) % CAPACITY;
        match self.slots[index] {
            Some(record @ ObservationRecord::Event(_)) => record,
            _ => unreachable!(),
        }
    }
}

/// A bounded read-only cursor. A synthetic gap, when present, is its first item.
pub struct ObservationDrain<'a, const CAPACITY: usize> {
    recorder: &'a ObservationRecorder<CAPACITY>,
    snapshot: SnapshotBounds,
    remaining: usize,
    gap: Option<ObservationGap>,
    next_offset: usize,
    next_cursor: u64,
}

impl<const CAPACITY: usize> ObservationDrain<'_, CAPACITY> {
    pub const fn snapshot(&self) -> SnapshotBounds {
        self.snapshot
    }

    /// The cursor to use for the next request after items already yielded.
    pub const fn next_cursor(&self) -> u64 {
        self.next_cursor
    }
}

impl<const CAPACITY: usize> Iterator for ObservationDrain<'_, CAPACITY> {
    type Item = ObservationRecord;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        if let Some(gap) = self.gap.take() {
            self.remaining -= 1;
            self.next_cursor = gap.first_missing + gap.count - 1;
            return Some(ObservationRecord::Gap(gap));
        }
        if self.next_offset >= self.recorder.len {
            return None;
        }
        let record = self.recorder.record_at_offset(self.next_offset);
        self.next_offset += 1;
        self.remaining -= 1;
        if let ObservationRecord::Event(event) = record {
            self.next_cursor = event.sequence;
        }
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(recorder: &mut ObservationRecorder<3>, uptime_ms: u64) -> u64 {
        recorder
            .record(
                uptime_ms,
                ObservationKind::ListeningStarted {
                    assignment: 7,
                    profile: 2,
                },
            )
            .unwrap()
    }

    fn request(after_sequence: u64, max_records: usize) -> CursorRequest {
        CursorRequest {
            boot_id: 9,
            after_sequence,
            max_records,
        }
    }

    #[test]
    fn rejects_zero_boot_and_zero_capacity() {
        assert!(matches!(
            ObservationRecorder::<1>::new(0),
            Err(RecorderInitError::InvalidBootId)
        ));
        let mut recorder = ObservationRecorder::<0>::new(9).unwrap();
        assert_eq!(
            recorder.record(0, ObservationKind::SleepStarted),
            Err(RecordError::CapacityZero)
        );
    }

    #[test]
    fn records_in_sequence_and_rejects_uptime_regression() {
        let mut recorder = ObservationRecorder::<3>::new(9).unwrap();
        assert_eq!(started(&mut recorder, 10), 1);
        assert_eq!(started(&mut recorder, 10), 2);
        assert_eq!(
            started_error(&mut recorder, 9),
            RecordError::UptimeRegressed {
                previous_ms: 10,
                current_ms: 9
            }
        );
        assert_eq!(
            recorder.stats(),
            RecorderStats {
                recorded: 2,
                overwritten: 0,
                encode_failed: 0
            }
        );
    }

    #[test]
    fn overwrite_emits_exact_gap_and_gap_counts_against_limit() {
        let mut recorder = ObservationRecorder::<3>::new(9).unwrap();
        for uptime in 0..5 {
            started(&mut recorder, uptime);
        }
        assert_eq!(recorder.bounds().oldest_available, Some(3));
        assert_eq!(recorder.stats().overwritten, 2);

        let mut one = recorder.drain(request(0, 1)).unwrap();
        assert_eq!(
            one.next(),
            Some(ObservationRecord::Gap(ObservationGap {
                boot_id: 9,
                first_missing: 1,
                count: 2
            }))
        );
        assert_eq!(one.next_cursor(), 2);
        assert_eq!(one.next(), None);

        let mut after_gap = recorder.drain(request(2, 3)).unwrap();
        assert_eq!(event_sequence(after_gap.next().unwrap()), 3);
        assert_eq!(event_sequence(after_gap.next().unwrap()), 4);
        assert_eq!(event_sequence(after_gap.next().unwrap()), 5);
        assert_eq!(after_gap.next_cursor(), 5);
    }

    #[test]
    fn repeated_cursor_is_idempotent_and_bounds_are_exposed() {
        let mut recorder = ObservationRecorder::<3>::new(9).unwrap();
        for uptime in 0..2 {
            started(&mut recorder, uptime);
        }
        let mut first_drain = recorder.drain(request(0, 3)).unwrap();
        let first = [
            event_sequence(first_drain.next().unwrap()),
            event_sequence(first_drain.next().unwrap()),
        ];
        let mut second_drain = recorder.drain(request(0, 3)).unwrap();
        let second = [
            event_sequence(second_drain.next().unwrap()),
            event_sequence(second_drain.next().unwrap()),
        ];
        assert_eq!(first, second);
        assert_eq!(
            recorder.drain(request(0, 3)).unwrap().snapshot(),
            SnapshotBounds {
                boot_id: 9,
                oldest_available: Some(1),
                newest_available: Some(2)
            }
        );
    }

    #[test]
    fn rejects_wrong_boot_and_future_cursor_and_handles_empty_ring() {
        let mut recorder = ObservationRecorder::<3>::new(9).unwrap();
        assert!(matches!(
            recorder.drain(CursorRequest {
                boot_id: 8,
                after_sequence: 0,
                max_records: 1
            }),
            Err(CursorError::WrongBoot {
                expected: 9,
                requested: 8
            })
        ));
        assert!(matches!(
            recorder.drain(request(1, 1)),
            Err(CursorError::FutureSequence {
                newest_available: None,
                requested: 1
            })
        ));
        assert_eq!(recorder.drain(request(0, 1)).unwrap().next(), None);
        started(&mut recorder, 0);
        assert!(matches!(
            recorder.drain(request(2, 1)),
            Err(CursorError::FutureSequence {
                newest_available: Some(1),
                requested: 2
            })
        ));
    }

    #[test]
    fn invalid_event_counts_encode_failure_without_consuming_sequence() {
        let mut recorder = ObservationRecorder::<3>::new(9).unwrap();
        let invalid = ObservationKind::Unknown {
            kind: 10,
            data: [0; super::super::UNKNOWN_DATA_MAX],
            len: 0,
        };
        assert_eq!(
            recorder.record(10, invalid),
            Err(RecordError::Encode(EncodeError::InvalidKind))
        );
        assert_eq!(recorder.stats().encode_failed, 1);
        assert_eq!(started(&mut recorder, 10), 1);
    }

    #[test]
    fn exhaustive_small_rings_bound_every_drain() {
        exercise_every_cursor::<1>();
        exercise_every_cursor::<2>();
        exercise_every_cursor::<3>();
    }

    #[test]
    fn sequence_exhaustion_does_not_wrap() {
        let mut recorder = ObservationRecorder::<1>::new(9).unwrap();
        recorder.next_sequence = Some(u64::MAX);
        assert_eq!(started_one(&mut recorder, 0), u64::MAX);
        assert_eq!(
            recorder.record(0, ObservationKind::SleepStarted),
            Err(RecordError::SequenceExhausted)
        );
        assert_eq!(recorder.bounds().newest_available, Some(u64::MAX));
    }

    #[test]
    fn counters_saturate() {
        let mut recorder = ObservationRecorder::<1>::new(9).unwrap();
        recorder.stats = RecorderStats {
            recorded: u64::MAX,
            overwritten: u64::MAX,
            encode_failed: u64::MAX,
        };
        started_one(&mut recorder, 0);
        started_one(&mut recorder, 1);
        let invalid = ObservationKind::Unknown {
            kind: 10,
            data: [0; super::super::UNKNOWN_DATA_MAX],
            len: 0,
        };
        assert!(matches!(
            recorder.record(1, invalid),
            Err(RecordError::Encode(EncodeError::InvalidKind))
        ));
        assert_eq!(
            recorder.stats(),
            RecorderStats {
                recorded: u64::MAX,
                overwritten: u64::MAX,
                encode_failed: u64::MAX,
            }
        );
    }

    fn started_error(recorder: &mut ObservationRecorder<3>, uptime_ms: u64) -> RecordError {
        recorder
            .record(uptime_ms, ObservationKind::SleepStarted)
            .unwrap_err()
    }

    fn started_one(recorder: &mut ObservationRecorder<1>, uptime_ms: u64) -> u64 {
        recorder
            .record(
                uptime_ms,
                ObservationKind::ListeningStarted {
                    assignment: 7,
                    profile: 2,
                },
            )
            .unwrap()
    }

    fn exercise_every_cursor<const CAPACITY: usize>() {
        let mut recorder = ObservationRecorder::<CAPACITY>::new(9).unwrap();
        for recorded in 0..=(CAPACITY * 3 + 1) {
            if recorded > 0 {
                recorder
                    .record(recorded as u64, ObservationKind::SleepStarted)
                    .unwrap();
            }
            assert_eq!(recorder.stats().recorded, recorded as u64);
            assert_eq!(
                recorder.stats().overwritten,
                recorded.saturating_sub(CAPACITY) as u64
            );
            let bounds = recorder.bounds();
            let newest = bounds.newest_available.unwrap_or(0);
            for after_sequence in 0..=newest {
                for max_records in 0..=(CAPACITY + 2) {
                    let mut drain = recorder
                        .drain(request(after_sequence, max_records))
                        .unwrap();
                    let mut emitted = 0;
                    let mut prior_cursor = after_sequence;
                    while let Some(record) = drain.next() {
                        emitted += 1;
                        assert!(emitted <= max_records);
                        match record {
                            ObservationRecord::Gap(gap) => {
                                assert_eq!(gap.first_missing, prior_cursor + 1);
                                assert_eq!(drain.next_cursor(), gap.first_missing + gap.count - 1);
                            }
                            ObservationRecord::Event(event) => {
                                assert_eq!(event.sequence, prior_cursor + 1);
                                assert_eq!(drain.next_cursor(), event.sequence);
                            }
                        }
                        prior_cursor = drain.next_cursor();
                    }
                    assert!(emitted <= CAPACITY.saturating_add(1));
                }
            }
            assert!(matches!(
                recorder.drain(request(newest + 1, 1)),
                Err(CursorError::FutureSequence { .. })
            ));
        }
    }

    fn event_sequence(record: ObservationRecord) -> u64 {
        match record {
            ObservationRecord::Event(event) => event.sequence,
            ObservationRecord::Gap(_) => panic!("expected only events"),
        }
    }
}
