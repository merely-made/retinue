use super::codec::crc32;
use super::*;
#[test]
fn crc_vector() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
}
#[test]
fn all_kinds_fit_and_roundtrip() {
    let kinds = [
        ObservationKind::ListeningStarted {
            assignment: 2,
            profile: 3,
        },
        ObservationKind::ListeningStopped {
            assignment: 2,
            reason: StopReason::Fault,
        },
        ObservationKind::RxCaptured {
            profile: 3,
            length: 4,
            rssi_dbm: -70,
            snr_tenths_db: 125,
            capture_tag: 9,
        },
        ObservationKind::RxDamaged { profile: 3 },
        ObservationKind::TxStarted {
            profile: 3,
            length: 4,
            work: 9,
        },
        ObservationKind::TxFinished {
            work: 9,
            outcome: TxOutcome::Sent,
        },
        ObservationKind::WorkRefused {
            request: RequestKind::Transmit,
            reason: RefusalReason::DutyBudget,
            work: 9,
        },
        ObservationKind::QuietStarted {
            cause: QuietCause::Configuration,
        },
        ObservationKind::QuietStopped {
            cause: QuietCause::Configuration,
        },
        ObservationKind::SleepStarted,
        ObservationKind::SleepStopped {
            cause: WakeCause::Timer,
        },
    ];
    for kind in kinds {
        let r = ObservationRecord::Event(ObservationEvent {
            boot_id: 1,
            sequence: 1,
            uptime_ms: 2,
            kind,
        });
        let mut b = [0; MAX_RECORD_BYTES];
        let n = r.encode(&mut b).unwrap();
        assert_eq!(ObservationRecord::decode(&b[..n]).unwrap(), r);
    }
}

#[test]
fn literal_event_fixtures_pin_every_known_kind() {
    let records = [
        (
            ObservationKind::ListeningStarted {
                assignment: 2,
                profile: 3,
            },
            &[
                79, 1, 0, 0, 37, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 0, 0, 2, 3, 199, 106, 211, 144,
            ][..],
        ),
        (
            ObservationKind::ListeningStopped {
                assignment: 2,
                reason: StopReason::Fault,
            },
            &[
                79, 1, 0, 0, 37, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 1, 0, 2, 2, 8, 209, 132, 99,
            ][..],
        ),
        (
            ObservationKind::RxCaptured {
                profile: 3,
                length: 4,
                rssi_dbm: -70,
                snr_tenths_db: 125,
                capture_tag: 9,
            },
            &[
                79, 1, 0, 0, 45, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 2, 3, 0, 4, 255, 186, 0, 125, 0, 0, 0, 9, 49, 232, 8, 113,
            ][..],
        ),
        (
            ObservationKind::RxDamaged { profile: 3 },
            &[
                79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 3, 3, 173, 72, 89, 35,
            ][..],
        ),
        (
            ObservationKind::TxStarted {
                profile: 3,
                length: 4,
                work: 9,
            },
            &[
                79, 1, 0, 0, 41, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 4, 3, 0, 4, 0, 0, 0, 9, 13, 96, 114, 34,
            ][..],
        ),
        (
            ObservationKind::TxFinished {
                work: 9,
                outcome: TxOutcome::Sent,
            },
            &[
                79, 1, 0, 0, 39, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 5, 0, 0, 0, 9, 0, 27, 205, 81, 107,
            ][..],
        ),
        (
            ObservationKind::WorkRefused {
                request: RequestKind::Transmit,
                reason: RefusalReason::DutyBudget,
                work: 9,
            },
            &[
                79, 1, 0, 0, 40, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 6, 0, 1, 0, 0, 0, 9, 41, 125, 12, 203,
            ][..],
        ),
        (
            ObservationKind::QuietStarted {
                cause: QuietCause::Configuration,
            },
            &[
                79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 7, 0, 80, 45, 205, 157,
            ][..],
        ),
        (
            ObservationKind::QuietStopped {
                cause: QuietCause::Configuration,
            },
            &[
                79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 8, 0, 215, 181, 209, 82,
            ][..],
        ),
        (
            ObservationKind::SleepStarted,
            &[
                79, 1, 0, 0, 34, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 9, 39, 115, 90, 198,
            ][..],
        ),
        (
            ObservationKind::SleepStopped {
                cause: WakeCause::Timer,
            },
            &[
                79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
                0, 2, 10, 0, 229, 131, 179, 208,
            ][..],
        ),
    ];
    for (kind, expected) in records {
        let mut bytes = [0; MAX_RECORD_BYTES];
        let n = ObservationRecord::Event(ObservationEvent {
            boot_id: 1,
            sequence: 1,
            uptime_ms: 2,
            kind,
        })
        .encode(&mut bytes)
        .unwrap();
        assert_eq!(&bytes[..n], expected);
        assert_eq!(
            ObservationRecord::decode(expected),
            Ok(ObservationRecord::Event(ObservationEvent {
                boot_id: 1,
                sequence: 1,
                uptime_ms: 2,
                kind
            }))
        );
    }
}

#[test]
fn boundary_unknown_and_raw_reasons_are_literal_and_lossless() {
    let mut data = [0u8; UNKNOWN_DATA_MAX];
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = i as u8;
    }
    let record = ObservationRecord::Event(ObservationEvent {
        boot_id: 9,
        sequence: 7,
        uptime_ms: 6,
        kind: ObservationKind::Unknown {
            kind: 200,
            data,
            len: UNKNOWN_DATA_MAX as u8,
        },
    });
    let mut bytes = [0u8; MAX_RECORD_BYTES];
    let n = record.encode(&mut bytes).unwrap();
    let expected = [
        79, 1, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 6,
        200, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
        24, 25, 26, 27, 28, 29, 235, 153, 238, 180,
    ];
    assert_eq!(&bytes[..n], &expected);
    assert_eq!(n, MAX_RECORD_BYTES);
    assert_eq!(ObservationRecord::decode(&bytes[..n]), Ok(record));
    let raw = ObservationRecord::Event(ObservationEvent {
        boot_id: 1,
        sequence: 1,
        uptime_ms: 1,
        kind: ObservationKind::ListeningStopped {
            assignment: 1,
            reason: StopReason::Unknown(99),
        },
    });
    let n = raw.encode(&mut bytes).unwrap();
    assert_eq!(ObservationRecord::decode(&bytes[..n]), Ok(raw));
}

#[test]
fn every_truncation_point_and_checksum_valid_payload_mutation_reject() {
    let record = ObservationRecord::Event(ObservationEvent {
        boot_id: 1,
        sequence: 1,
        uptime_ms: 0,
        kind: ObservationKind::SleepStarted,
    });
    let mut bytes = [0u8; MAX_RECORD_BYTES];
    let n = record.encode(&mut bytes).unwrap();
    for end in 0..n {
        assert!(
            ObservationRecord::decode(&bytes[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
    let mut malformed = [0u8; MAX_RECORD_BYTES];
    malformed[..n].copy_from_slice(&bytes[..n]);
    malformed[n - 4] = 0; // append one payload byte before the checksum
    malformed[4] += 1;
    let crc = crc32(&malformed[..n - 3]);
    malformed[n - 3..n + 1].copy_from_slice(&crc.to_be_bytes());
    assert_eq!(
        ObservationRecord::decode(&malformed[..n + 1]),
        Err(DecodeError::InvalidPayload)
    );
}

#[test]
fn zero_identity_sequence_and_gap_end_overflow_are_rejected_after_checksum() {
    let record = ObservationRecord::Event(ObservationEvent {
        boot_id: 1,
        sequence: 1,
        uptime_ms: 0,
        kind: ObservationKind::SleepStarted,
    });
    let mut bytes = [0u8; MAX_RECORD_BYTES];
    let n = record.encode(&mut bytes).unwrap();
    bytes[5..13].fill(0);
    let crc = crc32(&bytes[..n - 4]);
    bytes[n - 4..n].copy_from_slice(&crc.to_be_bytes());
    assert_eq!(
        ObservationRecord::decode(&bytes[..n]),
        Err(DecodeError::InvalidIdentity)
    );
    let n = record.encode(&mut bytes).unwrap();
    bytes[13..21].fill(0);
    let crc = crc32(&bytes[..n - 4]);
    bytes[n - 4..n].copy_from_slice(&crc.to_be_bytes());
    assert_eq!(
        ObservationRecord::decode(&bytes[..n]),
        Err(DecodeError::InvalidSequence)
    );
    assert_eq!(
        ObservationRecord::Gap(ObservationGap {
            boot_id: 1,
            first_missing: u64::MAX,
            count: 2
        })
        .encode(&mut bytes),
        Err(EncodeError::InvalidRange)
    );
}
#[test]
fn unknown_and_gap_roundtrip() {
    let mut data = [0; UNKNOWN_DATA_MAX];
    data[..3].copy_from_slice(&[1, 2, 3]);
    let r = ObservationRecord::Event(ObservationEvent {
        boot_id: 2,
        sequence: 3,
        uptime_ms: 4,
        kind: ObservationKind::Unknown {
            kind: 99,
            data,
            len: 3,
        },
    });
    let mut b = [0; MAX_RECORD_BYTES];
    let n = r.encode(&mut b).unwrap();
    assert_eq!(ObservationRecord::decode(&b[..n]).unwrap(), r);
    let g = ObservationRecord::Gap(ObservationGap {
        boot_id: 2,
        first_missing: 8,
        count: 4,
    });
    let n = g.encode(&mut b).unwrap();
    let expected = [
        79, 1, 1, 0, 33, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 4,
        51, 74, 249, 166,
    ];
    assert_eq!(&b[..n], &expected);
    assert_eq!(ObservationRecord::decode(&b[..n]).unwrap(), g);
}

#[test]
fn continuity_loss_has_an_independent_literal() {
    // Python struct.pack big-endian fields and zlib.crc32, independent of
    // this encoder. A loss marker asserts neither standby nor its time.
    let literal = [
        79, 1, 0, 0, 35, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 6,
        11, 3, 131, 140, 172, 49,
    ];
    let record = ObservationRecord::Event(ObservationEvent {
        boot_id: 9,
        sequence: 7,
        uptime_ms: 6,
        kind: ObservationKind::ContinuityLost { cause: 3 },
    });
    assert_eq!(ObservationRecord::decode(&literal), Ok(record));
    let mut bytes = [0; MAX_RECORD_BYTES];
    let len = record.encode(&mut bytes).unwrap();
    assert_eq!(&bytes[..len], &literal);
}
#[test]
fn unknown_empty_payload_is_preserved() {
    let record = ObservationRecord::Event(ObservationEvent {
        boot_id: 1,
        sequence: u64::MAX,
        uptime_ms: 0,
        kind: ObservationKind::Unknown {
            kind: 200,
            data: [0; UNKNOWN_DATA_MAX],
            len: 0,
        },
    });
    let mut bytes = [0; MAX_RECORD_BYTES];
    let len = record.encode(&mut bytes).unwrap();
    assert_eq!(len, 34);
    assert_eq!(ObservationRecord::decode(&bytes[..len]), Ok(record));
}

#[test]
fn rejects_trailing_truncated_and_bad_values() {
    let r = ObservationRecord::Event(ObservationEvent {
        boot_id: 1,
        sequence: 1,
        uptime_ms: 0,
        kind: ObservationKind::SleepStarted,
    });
    let mut b = [0; MAX_RECORD_BYTES];
    let n = r.encode(&mut b).unwrap();
    assert_eq!(
        ObservationRecord::decode(&b[..n - 1]),
        Err(DecodeError::Truncated)
    );
    assert_eq!(
        ObservationRecord::decode(&b[..n + 1]),
        Err(DecodeError::BadLength)
    );
    b[4] = 0;
    assert_eq!(
        ObservationRecord::decode(&b[..n]),
        Err(DecodeError::BadLength)
    );
}
