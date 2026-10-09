//! Consumer checks across the real RAM recorder, wire codec, and host reducer.
use radio_hand::observation::{
    MAX_RECORD_BYTES, ObservationKind, ObservationRecord, StopReason,
    recorder::{CursorRequest, ObservationRecorder},
};
use signalman::observation::{
    Activity, Admission, AdmissionError, BUNDLE_VERSION, BundleEntry, CarrierKind, Edge,
    IncompleteReason, ObservationBundle, ProfileEntry, ReplayError, replay,
};

mod collection;
mod intervals;

fn bundle() -> ObservationBundle {
    ObservationBundle::new(
        BUNDLE_VERSION,
        b"bench-board",
        CarrierKind::LocalUsb,
        "fixture-usb",
        &[profile()],
        Admission {
            max_frames: 32,
            max_bytes: 4096,
        },
    )
    .unwrap()
}

fn admit(bundle: &mut ObservationBundle, record: ObservationRecord, received: u64) {
    let mut raw = [0; MAX_RECORD_BYTES];
    let length = record.encode(&mut raw).unwrap();
    bundle.admit(&raw[..length], received).unwrap();
}

fn profile() -> ProfileEntry {
    ProfileEntry {
        id: 2,
        name: "fixture".into(),
        version: 1,
        definition:
            b"fixture-v1:915000000Hz;SF7;BW125000;CR4/5;preamble8;sync18;explicit;crc;iq-normal"
                .to_vec(),
    }
}

fn listening() -> ObservationKind {
    ObservationKind::ListeningStarted {
        assignment: 7,
        profile: 2,
    }
}

fn stopped() -> ObservationKind {
    ObservationKind::ListeningStopped {
        assignment: 7,
        reason: StopReason::Completed,
    }
}

fn event(sequence: u64, uptime_ms: u64, kind: ObservationKind) -> ObservationRecord {
    ObservationRecord::Event(radio_hand::observation::ObservationEvent {
        boot_id: 9,
        sequence,
        uptime_ms,
        kind,
    })
}
