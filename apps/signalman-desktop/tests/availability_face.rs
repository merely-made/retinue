use cambium_genet_winit_host::{Harness, Init, inert_hooks};
use genet_probe::Selector;
use radio_hand::observation::{
    MAX_RECORD_BYTES, ObservationEvent, ObservationGap, ObservationKind, ObservationRecord,
    RefusalReason, RequestKind, StopReason, TxOutcome,
};
use signalman::observation::persistence::{
    DurableCapture, Retention, StoreOutcome, StoredCapture, encode_stored,
};
use signalman::observation::{
    Admission, BUNDLE_VERSION, CarrierKind, ObservationBundle, ProfileEntry, replay,
};
use signalman_desktop::availability::{
    AvailabilityCapture, AvailabilitySettings, accept_live_bundle, accept_live_capture,
    export_capture, load_capture, load_settings, save_settings,
};
use signalman_desktop::state::{DesktopSection, DesktopState, ObservationRequest};
use signalman_desktop::views::Logic;
use signalman_desktop::{SHEET, default_catalog_path, root};

fn admit(
    bundle: &mut ObservationBundle,
    boot_id: u64,
    sequence: u64,
    uptime_ms: u64,
    kind: ObservationKind,
) {
    let mut raw = [0; MAX_RECORD_BYTES];
    let length = ObservationRecord::Event(ObservationEvent {
        boot_id,
        sequence,
        uptime_ms,
        kind,
    })
    .encode(&mut raw)
    .unwrap();
    bundle.admit(&raw[..length], 10_000 + uptime_ms).unwrap();
}

fn capture(device: &str, second: bool) -> AvailabilityCapture {
    let admission = Admission {
        max_frames: 16,
        max_bytes: 4096,
    };
    let mut bundle = ObservationBundle::new(
        BUNDLE_VERSION,
        device.as_bytes(),
        CarrierKind::Imported,
        "headed-fixture",
        &[ProfileEntry {
            id: 1,
            version: 1,
            name: "US915".into(),
            definition: b"selvage-config-v1:fixture".to_vec(),
        }],
        admission,
    )
    .unwrap();
    if second {
        let mut raw = [0; MAX_RECORD_BYTES];
        let length = ObservationRecord::Gap(ObservationGap {
            boot_id: 20,
            first_missing: 1,
            count: 2,
        })
        .encode(&mut raw)
        .unwrap();
        bundle.admit(&raw[..length], 10_000).unwrap();
        admit(
            &mut bundle,
            20,
            3,
            50,
            ObservationKind::TxStarted {
                work: 8,
                profile: 1,
                length: 12,
            },
        );
        admit(
            &mut bundle,
            20,
            4,
            70,
            ObservationKind::TxFinished {
                work: 8,
                outcome: TxOutcome::Sent,
            },
        );
        admit(
            &mut bundle,
            20,
            5,
            80,
            ObservationKind::WorkRefused {
                request: RequestKind::Transmit,
                reason: RefusalReason::ChannelBusy,
                work: 9,
            },
        );
    } else {
        admit(
            &mut bundle,
            10,
            1,
            100,
            ObservationKind::ListeningStarted {
                assignment: 4,
                profile: 1,
            },
        );
        admit(
            &mut bundle,
            10,
            2,
            130,
            ObservationKind::ListeningStopped {
                assignment: 4,
                reason: StopReason::Completed,
            },
        );
        admit(
            &mut bundle,
            11,
            1,
            5,
            ObservationKind::ListeningStarted {
                assignment: 4,
                profile: 1,
            },
        );
    }
    let timeline = replay(&bundle).unwrap();
    AvailabilityCapture {
        source: format!("fixture:{device}"),
        stored: StoredCapture {
            captured_unix_ms: 20_000,
            retention: Retention {
                max_entries: 16,
                max_payload_bytes: 4096,
                max_age_ms: None,
            },
            omitted_prefix_entries: usize::from(second),
            bundle,
        },
        timeline,
    }
}

#[test]
fn normal_navigation_shows_two_board_timeline_and_uncertainty() {
    let mut state = DesktopState::new(&default_catalog_path());
    let supplied = [capture("v4-wall", false), capture("t114-field", true)];
    if let Some(directory) = std::env::var_os("SIGNALMAN_WRITE_AVAILABILITY_FIXTURES") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        for capture in &supplied {
            let name = String::from_utf8_lossy(capture.stored.bundle.device());
            std::fs::write(
                directory.join(format!("{name}.json")),
                encode_stored(&capture.stored).unwrap(),
            )
            .unwrap();
        }
    }
    for supplied in supplied {
        let (projected, outcome) =
            accept_live_capture(supplied.source, supplied.stored, &DurableCapture::Disabled)
                .unwrap();
        assert_eq!(outcome.unwrap(), StoreOutcome::Disabled);
        state.adopt_availability(projected);
    }
    let mut harness = Harness::with_hooks(
        Init {
            state,
            logic: root as Logic,
            sheet: SHEET.to_owned(),
        },
        inert_hooks(),
    );
    harness.layout_at(1100.0, 800.0);
    assert!(harness.click_on(&Selector::role("button").containing("Radio")));
    assert_eq!(harness.state().section, DesktopSection::Radio);
    harness.with_surfaces(|surfaces| {
        for text in [
            "Radio availability",
            "v4-wall",
            "t114-field",
            "transmitting",
            "1 refusals",
            "missing source records",
            "boot 11",
        ] {
            assert!(genet_probe::text_present(surfaces, text), "missing {text}");
        }
    });
    assert!(harness.click_on(&Selector::role("button").containing("Turn durable capture on")));
    assert!(harness.state().observation_durable);
    assert!(harness.click_on(&Selector::role("button").containing("Turn durable capture off")));
    assert!(!harness.state().observation_durable);
    assert!(harness.click_on(&Selector::role("button").containing("Export selected capture")));
    let mut request = None;
    harness.update(|state| request = state.take_observation_request());
    assert_eq!(request, Some(ObservationRequest::Export));
    assert_eq!(
        harness.state().availability.len(),
        2,
        "live rendering survives the toggle"
    );
}

#[test]
fn real_load_and_export_preserve_the_replay_envelope_and_refuse_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let original = capture("v4-wall", false);
    let input = root.path().join("input.json");
    std::fs::write(&input, encode_stored(&original.stored).unwrap()).unwrap();
    let loaded = load_capture(&input).unwrap();
    assert_eq!(loaded.timeline, original.timeline);
    let output = root.path().join("output.json");
    export_capture(&output, &loaded).unwrap();
    assert_eq!(
        std::fs::read(&output).unwrap(),
        encode_stored(&loaded.stored).unwrap()
    );
    let before = std::fs::read(&output).unwrap();
    assert!(export_capture(&output, &loaded).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), before);
}

#[test]
fn owner_retention_and_durability_settings_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("availability-settings.json");
    let settings = AvailabilitySettings {
        durable: false,
        retention_entries: 1_024,
        retention_bytes: 131_072,
        retention_age_ms: 86_400_000,
    };
    save_settings(&path, settings).unwrap();
    assert_eq!(load_settings(&path).unwrap(), settings);
    let replacement = AvailabilitySettings {
        durable: true,
        retention_entries: 4_096,
        retention_bytes: 524_288,
        retention_age_ms: 604_800_000,
    };
    save_settings(&path, replacement).unwrap();
    assert_eq!(load_settings(&path).unwrap(), replacement);
    std::fs::write(
        &path,
        br#"{"schema_version":2,"durable":true,"retention_entries":1,"retention_bytes":1,"retention_age_ms":1}"#,
    )
    .unwrap();
    assert!(load_settings(&path).is_err());
    std::fs::write(
        &path,
        br#"{"schema_version":1,"durable":true,"retention_entries":0,"retention_bytes":1,"retention_age_ms":1}"#,
    )
    .unwrap();
    assert!(load_settings(&path).is_err());
}

#[test]
fn durable_failure_keeps_the_live_projection() {
    let root = tempfile::tempdir().unwrap();
    let occupied = root.path().join("occupied.json");
    std::fs::write(&occupied, b"owner data").unwrap();
    let supplied = capture("v4-wall", false);
    let (projected, durable) = accept_live_capture(
        supplied.source.clone(),
        supplied.stored.clone(),
        &DurableCapture::CreateNew(occupied.clone()),
    )
    .unwrap();
    assert_eq!(projected.timeline, supplied.timeline);
    assert!(durable.is_err());
    assert_eq!(std::fs::read(occupied).unwrap(), b"owner data");

    let missing_parent = root.path().join("missing").join("capture.json");
    let (projected, durable) = accept_live_capture(
        supplied.source,
        supplied.stored,
        &DurableCapture::CreateNew(missing_parent),
    )
    .unwrap();
    assert_eq!(projected.timeline, supplied.timeline);
    assert!(durable.is_err());
}

#[test]
fn live_bundle_uses_current_owner_bounds_before_projection() {
    let supplied = capture("t114-field", true);
    let settings = AvailabilitySettings {
        durable: false,
        retention_entries: 1,
        retention_bytes: 4_096,
        retention_age_ms: 20_000,
    };
    let (projected, outcome) = accept_live_bundle(
        "collector:v4-wall".into(),
        &supplied.stored.bundle,
        20_000,
        settings,
        &DurableCapture::Disabled,
        supplied.stored.omitted_prefix_entries,
    )
    .unwrap();
    assert_eq!(outcome.unwrap(), StoreOutcome::Disabled);
    assert_eq!(projected.stored.retention.max_entries, 1);
    assert_eq!(projected.stored.bundle.entries().len(), 1);
    assert!(
        projected.stored.omitted_prefix_entries > supplied.stored.omitted_prefix_entries,
        "new retention loss is added to the source-reported omitted prefix"
    );
}
