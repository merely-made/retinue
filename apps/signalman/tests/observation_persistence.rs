use radio_hand::observation::{
    MAX_RECORD_BYTES, ObservationEvent, ObservationKind, ObservationRecord, StopReason,
};
use signalman::observation::persistence::{
    DurableCapture, PersistenceError, ReadLimits, Retention, StoreOutcome, decode, encode,
    encode_stored, read, store,
};
use signalman::observation::{
    Admission, BUNDLE_VERSION, CarrierKind, ObservationBundle, ProfileEntry, replay,
};

const ADMISSION: Admission = Admission {
    max_frames: 32,
    max_bytes: 4096,
};

fn bundle() -> ObservationBundle {
    ObservationBundle::new(
        BUNDLE_VERSION,
        b"bench-board",
        CarrierKind::LocalUsb,
        "COM10",
        &[ProfileEntry {
            id: 2,
            version: 1,
            name: "US915 fixture".into(),
            definition: b"selvage-config-v1:fixture".to_vec(),
        }],
        ADMISSION,
    )
    .unwrap()
}

fn admit(bundle: &mut ObservationBundle, sequence: u64, time: u64, kind: ObservationKind) {
    let record = ObservationRecord::Event(ObservationEvent {
        boot_id: 9,
        sequence,
        uptime_ms: time,
        kind,
    });
    let mut raw = [0; MAX_RECORD_BYTES];
    let length = record.encode(&mut raw).unwrap();
    bundle.admit(&raw[..length], 1_000 + time).unwrap();
}

fn complete_bundle() -> ObservationBundle {
    let mut capture = bundle();
    admit(
        &mut capture,
        1,
        100,
        ObservationKind::ListeningStarted {
            assignment: 7,
            profile: 2,
        },
    );
    admit(
        &mut capture,
        2,
        130,
        ObservationKind::ListeningStopped {
            assignment: 7,
            reason: StopReason::Completed,
        },
    );
    capture.disconnect(1_200).unwrap();
    capture
}

fn retention() -> Retention {
    Retention {
        max_entries: 32,
        max_payload_bytes: 4096,
        max_age_ms: None,
    }
}

fn limits(max_file_bytes: usize) -> ReadLimits {
    ReadLimits {
        max_file_bytes,
        admission: ADMISSION,
    }
}

#[test]
fn reload_preserves_source_evidence_and_deterministic_replay() {
    let capture = complete_bundle();
    let bytes = encode(&capture, 2_000, retention()).unwrap();
    let reopened = decode(&bytes, limits(bytes.len())).unwrap();
    assert_eq!(reopened.bundle, capture);
    assert_eq!(replay(&reopened.bundle).unwrap(), replay(&capture).unwrap());
    assert_eq!(encode_stored(&reopened).unwrap(), bytes);
}

#[test]
fn independently_written_literal_container_replays_exact_source_bytes() {
    let literal = br#"{
      "schema_version":1,"bundle_version":1,"captured_unix_ms":2000,
      "retention":{"max_entries":8,"max_payload_bytes":4096,"max_age_ms":null},
      "omitted_prefix_entries":0,"device_hex":"62656e63682d626f617264",
      "carrier":{"kind":"local-usb"},"carrier_label":"fixture",
      "profiles":[{"id":2,"version":1,"name":"fixture","definition_hex":"66697874757265"}],
      "entries":[
        {"kind":"record","received_unix_ms":1100,"raw_hex":"4f0100002500000000000000090000000000000001000000000000006400000702fe9a9cd8"},
        {"kind":"record","received_unix_ms":1130,"raw_hex":"4f0100002500000000000000090000000000000002000000000000008201000700b276c420"}
      ]
    }"#;
    let reopened = decode(literal, limits(literal.len())).unwrap();
    assert_eq!(reopened.bundle.entries().len(), 2);
    assert_eq!(
        replay(&reopened.bundle).unwrap().summary.listening_ms[&2],
        30
    );
}

#[test]
fn corruption_unknown_fields_and_read_bounds_are_refused() {
    let capture = complete_bundle();
    let bytes = encode(&capture, 2_000, retention()).unwrap();
    assert!(matches!(
        decode(&bytes, limits(bytes.len() - 1)),
        Err(PersistenceError::FileTooLarge)
    ));

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["entries"][0]["raw_hex"] = serde_json::Value::String("00".into());
    let corrupt = serde_json::to_vec(&value).unwrap();
    assert!(matches!(
        decode(&corrupt, limits(corrupt.len())),
        Err(PersistenceError::Admission(_))
    ));

    value["unexpected"] = serde_json::Value::Bool(true);
    let unknown = serde_json::to_vec(&value).unwrap();
    assert!(matches!(
        decode(&unknown, limits(unknown.len())),
        Err(PersistenceError::Json(_))
    ));

    let mut undersized: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    undersized["retention"]["max_payload_bytes"] = serde_json::Value::from(1);
    let undersized = serde_json::to_vec(&undersized).unwrap();
    assert!(matches!(
        decode(&undersized, limits(undersized.len())),
        Err(PersistenceError::Admission(_))
    ));
}

#[test]
fn retention_keeps_a_bounded_newest_suffix_and_exposes_the_gap() {
    let mut capture = complete_bundle();
    admit(
        &mut capture,
        3,
        300,
        ObservationKind::RxDamaged { profile: 2 },
    );
    let policy = Retention {
        max_entries: 1,
        max_payload_bytes: 4096,
        max_age_ms: None,
    };
    let bytes = encode(&capture, 2_000, policy).unwrap();
    let reopened = decode(&bytes, limits(bytes.len())).unwrap();
    assert_eq!(reopened.omitted_prefix_entries, 3);
    assert_eq!(reopened.bundle.entries().len(), 1);
    let timeline = replay(&reopened.bundle).unwrap();
    assert_eq!(timeline.summary.missing_records, 2);
    assert_eq!(timeline.summary.damaged, 1);
    let exported_again = encode_stored(&reopened).unwrap();
    let reopened_again = decode(&exported_again, limits(exported_again.len())).unwrap();
    assert_eq!(reopened_again.omitted_prefix_entries, 3);
    assert_eq!(reopened_again.bundle.entries().len(), 1);
}

#[test]
fn age_retention_and_disabled_durability_are_explicit() {
    let capture = complete_bundle();
    let policy = Retention {
        max_entries: 32,
        max_payload_bytes: 4096,
        max_age_ms: Some(880),
    };
    let bytes = encode(&capture, 2_000, policy).unwrap();
    let reopened = decode(&bytes, limits(bytes.len())).unwrap();
    assert_eq!(reopened.omitted_prefix_entries, 1);
    assert_eq!(reopened.bundle.entries().len(), 2);

    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("disabled.json");
    assert_eq!(
        store(&DurableCapture::Disabled, &capture, 2_000, retention()).unwrap(),
        StoreOutcome::Disabled
    );
    assert!(!absent.exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);

    let path = root.path().join("capture.json");
    let outcome = store(
        &DurableCapture::CreateNew(path.clone()),
        &capture,
        2_000,
        retention(),
    )
    .unwrap();
    assert!(matches!(outcome, StoreOutcome::Written { entries: 3, .. }));
    let published = std::fs::read(&path).unwrap();
    let reopened = read(&path, limits(16 * 1024)).unwrap();
    assert_eq!(reopened.bundle, capture);
    assert_eq!(reopened.retention, retention());
    assert!(matches!(
        store(
            &DurableCapture::CreateNew(path.clone()),
            &capture,
            2_000,
            retention()
        ),
        Err(PersistenceError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(std::fs::read(&path).unwrap(), published);
    let names: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("capture.json")]);
}
