//! Signalman's owner-facing projection of stored radio observations.

use signalman::observation::persistence::{
    DurableCapture, PersistenceError, ReadLimits, Retention, StoreOutcome, StoredCapture,
    encode_stored, read, retained_capture, store, write_stored,
};
use signalman::observation::{Admission, ObservationBundle, Timeline, replay};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvailabilitySettings {
    pub durable: bool,
    pub retention_entries: usize,
    pub retention_bytes: usize,
    pub retention_age_ms: u64,
}

impl Default for AvailabilitySettings {
    fn default() -> Self {
        Self {
            durable: false,
            retention_entries: 4096,
            retention_bytes: 512 * 1024,
            retention_age_ms: 7 * 24 * 60 * 60 * 1000,
        }
    }
}

pub const MAX_CAPTURE_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CAPTURE_ENTRIES: usize = 32 * 1024;
pub const MAX_CAPTURE_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailabilityCapture {
    pub source: String,
    pub stored: StoredCapture,
    pub timeline: Timeline,
}

pub fn load_capture(path: &Path) -> Result<AvailabilityCapture, String> {
    let stored = read(
        path,
        ReadLimits {
            max_file_bytes: MAX_CAPTURE_FILE_BYTES,
            admission: Admission {
                max_frames: MAX_CAPTURE_ENTRIES,
                max_bytes: MAX_CAPTURE_PAYLOAD_BYTES,
            },
        },
    )
    .map_err(persistence_message)?;
    let timeline = replay(&stored.bundle).map_err(|error| format!("replay refused: {error:?}"))?;
    Ok(AvailabilityCapture {
        source: path.display().to_string(),
        stored,
        timeline,
    })
}

pub fn export_capture(path: &Path, capture: &AvailabilityCapture) -> Result<(), String> {
    write_stored(path, &capture.stored).map_err(persistence_message)
}

/// Admit one collector-supplied capture into the live projection and optional
/// durable path. Disabled durability executes the storage API's zero-write path.
pub fn accept_live_capture(
    source: String,
    stored: StoredCapture,
    destination: &DurableCapture,
) -> Result<(AvailabilityCapture, Result<StoreOutcome, String>), String> {
    let timeline = replay(&stored.bundle).map_err(|error| format!("replay refused: {error:?}"))?;
    let outcome = match destination {
        DurableCapture::Disabled => store(
            destination,
            &stored.bundle,
            stored.captured_unix_ms,
            stored.retention,
        )
        .map_err(persistence_message),
        DurableCapture::CreateNew(path) => encode_stored(&stored)
            .map_err(persistence_message)
            .and_then(|bytes| {
                write_stored(path, &stored)
                    .map_err(persistence_message)
                    .map(|()| StoreOutcome::Written {
                        bytes: bytes.len(),
                        entries: stored.bundle.entries().len(),
                    })
            }),
    };
    Ok((
        AvailabilityCapture {
            source,
            stored,
            timeline,
        },
        outcome,
    ))
}

/// Apply the owner's current bounds to a collector bundle before the same
/// retained envelope is projected and optionally written.
pub fn accept_live_bundle(
    source: String,
    bundle: &ObservationBundle,
    captured_unix_ms: u64,
    settings: AvailabilitySettings,
    destination: &DurableCapture,
    prior_omitted_prefix_entries: usize,
) -> Result<(AvailabilityCapture, Result<StoreOutcome, String>), String> {
    let stored = retained_capture(
        bundle,
        captured_unix_ms,
        Retention {
            max_entries: settings.retention_entries,
            max_payload_bytes: settings.retention_bytes,
            max_age_ms: Some(settings.retention_age_ms),
        },
        prior_omitted_prefix_entries,
    )
    .map_err(persistence_message)?;
    accept_live_capture(source, stored, destination)
}

fn persistence_message(error: PersistenceError) -> String {
    format!("capture refused: {error}")
}

pub fn load_settings(path: &Path) -> Result<AvailabilitySettings, String> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if value["schema_version"].as_u64() != Some(1) {
        return Err("unsupported availability settings schema".into());
    }
    let settings = AvailabilitySettings {
        durable: value["durable"]
            .as_bool()
            .ok_or_else(|| "missing durable".to_owned())?,
        retention_entries: value["retention_entries"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| "invalid retention_entries".to_owned())?,
        retention_bytes: value["retention_bytes"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| "invalid retention_bytes".to_owned())?,
        retention_age_ms: value["retention_age_ms"]
            .as_u64()
            .ok_or_else(|| "invalid retention_age_ms".to_owned())?,
    };
    if settings.retention_entries == 0 || settings.retention_bytes == 0 {
        return Err("retention bounds must be nonzero".into());
    }
    Ok(settings)
}

pub fn save_settings(path: &Path, settings: AvailabilitySettings) -> Result<(), String> {
    if settings.retention_entries == 0 || settings.retention_bytes == 0 {
        return Err("retention bounds must be nonzero".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": 1,
        "durable": settings.durable,
        "retention_entries": settings.retention_entries,
        "retention_bytes": settings.retention_bytes,
        "retention_age_ms": settings.retention_age_ms,
    }))
    .map_err(|e| e.to_string())?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    use std::io::Write as _;
    temporary.write_all(&bytes).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| error.error.to_string())?;
    if let Ok(directory) = std::fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}
