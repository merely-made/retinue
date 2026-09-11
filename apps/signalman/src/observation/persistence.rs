//! Bounded, versioned storage for immutable observation captures.

use super::{
    Admission, AdmissionError, BUNDLE_VERSION, BundleEntry, CarrierKind, ObservationBundle,
    ProfileEntry,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const DISK_SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    pub max_entries: usize,
    pub max_payload_bytes: usize,
    pub max_age_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadLimits {
    pub max_file_bytes: usize,
    pub admission: Admission,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DurableCapture {
    Disabled,
    CreateNew(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOutcome {
    Disabled,
    Written { bytes: usize, entries: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCapture {
    pub captured_unix_ms: u64,
    pub retention: Retention,
    pub omitted_prefix_entries: usize,
    pub bundle: ObservationBundle,
}

#[derive(Debug)]
pub enum PersistenceError {
    Io(io::Error),
    Json(serde_json::Error),
    UnsupportedSchema,
    InvalidHex,
    Admission(AdmissionError),
    FileTooLarge,
    RetentionTooSmall,
    OmissionCountOverflow,
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for PersistenceError {}

impl From<io::Error> for PersistenceError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for PersistenceError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<AdmissionError> for PersistenceError {
    fn from(value: AdmissionError) -> Self {
        Self::Admission(value)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskBundle {
    schema_version: u8,
    bundle_version: u8,
    captured_unix_ms: u64,
    retention: DiskRetention,
    omitted_prefix_entries: usize,
    device_hex: String,
    carrier: DiskCarrier,
    carrier_label: String,
    profiles: Vec<DiskProfile>,
    entries: Vec<DiskEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskRetention {
    max_entries: usize,
    max_payload_bytes: usize,
    max_age_ms: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "code", rename_all = "kebab-case")]
enum DiskCarrier {
    LocalUsb,
    Imported,
    Other(u8),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskProfile {
    id: u8,
    version: u32,
    name: String,
    definition_hex: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum DiskEntry {
    Record {
        received_unix_ms: u64,
        raw_hex: String,
    },
    Disconnected {
        received_unix_ms: u64,
    },
}

fn carrier_to_disk(value: CarrierKind) -> DiskCarrier {
    match value {
        CarrierKind::LocalUsb => DiskCarrier::LocalUsb,
        CarrierKind::Imported => DiskCarrier::Imported,
        CarrierKind::Other(code) => DiskCarrier::Other(code),
    }
}

fn carrier_from_disk(value: DiskCarrier) -> CarrierKind {
    match value {
        DiskCarrier::LocalUsb => CarrierKind::LocalUsb,
        DiskCarrier::Imported => CarrierKind::Imported,
        DiskCarrier::Other(code) => CarrierKind::Other(code),
    }
}

fn retained_entries(
    bundle: &ObservationBundle,
    now_unix_ms: u64,
    retention: Retention,
) -> Result<Vec<&BundleEntry>, PersistenceError> {
    if retention.max_entries == 0 || retention.max_payload_bytes == 0 {
        return Err(PersistenceError::RetentionTooSmall);
    }
    let cutoff = retention
        .max_age_ms
        .map(|age| now_unix_ms.saturating_sub(age));
    let metadata_bytes = bundle.payload_bytes().saturating_sub(
        bundle
            .entries()
            .iter()
            .map(entry_payload_bytes)
            .sum::<usize>(),
    );
    if metadata_bytes > retention.max_payload_bytes {
        return Err(PersistenceError::RetentionTooSmall);
    }
    let mut bytes = metadata_bytes;
    let mut kept = Vec::new();
    for entry in bundle.entries().iter().rev() {
        if cutoff.is_some_and(|minimum| entry_time(entry) < minimum) {
            break;
        }
        let size = entry_payload_bytes(entry);
        if kept.len() == retention.max_entries
            || bytes.saturating_add(size) > retention.max_payload_bytes
        {
            break;
        }
        bytes += size;
        kept.push(entry);
    }
    kept.reverse();
    Ok(kept)
}

fn entry_time(entry: &BundleEntry) -> u64 {
    match entry {
        BundleEntry::Record(record) => record.received_unix_ms,
        BundleEntry::Disconnected { received_unix_ms } => *received_unix_ms,
    }
}

fn entry_payload_bytes(entry: &BundleEntry) -> usize {
    match entry {
        BundleEntry::Record(record) => record.raw.len() + 8,
        BundleEntry::Disconnected { .. } => 8,
    }
}

pub fn encode(
    bundle: &ObservationBundle,
    now_unix_ms: u64,
    retention: Retention,
) -> Result<Vec<u8>, PersistenceError> {
    let entries = retained_entries(bundle, now_unix_ms, retention)?;
    let omitted_prefix_entries = bundle.entries().len() - entries.len();
    let disk = DiskBundle {
        schema_version: DISK_SCHEMA_VERSION,
        bundle_version: bundle.version(),
        captured_unix_ms: now_unix_ms,
        retention: DiskRetention {
            max_entries: retention.max_entries,
            max_payload_bytes: retention.max_payload_bytes,
            max_age_ms: retention.max_age_ms,
        },
        omitted_prefix_entries,
        device_hex: hex::encode(bundle.device()),
        carrier: carrier_to_disk(bundle.carrier()),
        carrier_label: bundle.carrier_label().to_owned(),
        profiles: bundle
            .profiles()
            .iter()
            .map(|profile| DiskProfile {
                id: profile.id,
                version: profile.version,
                name: profile.name.clone(),
                definition_hex: hex::encode(&profile.definition),
            })
            .collect(),
        entries: entries
            .into_iter()
            .map(|entry| match entry {
                BundleEntry::Record(record) => DiskEntry::Record {
                    received_unix_ms: record.received_unix_ms,
                    raw_hex: hex::encode(&record.raw),
                },
                BundleEntry::Disconnected { received_unix_ms } => DiskEntry::Disconnected {
                    received_unix_ms: *received_unix_ms,
                },
            })
            .collect(),
    };
    Ok(serde_json::to_vec_pretty(&disk)?)
}

/// Apply retention once and return the exact envelope used for projection,
/// persistence, and later export.
pub fn retained_capture(
    bundle: &ObservationBundle,
    now_unix_ms: u64,
    retention: Retention,
    prior_omitted_prefix_entries: usize,
) -> Result<StoredCapture, PersistenceError> {
    let bytes = encode(bundle, now_unix_ms, retention)?;
    let mut stored = decode(
        &bytes,
        ReadLimits {
            max_file_bytes: bytes.len(),
            admission: Admission {
                max_frames: retention.max_entries,
                max_bytes: retention.max_payload_bytes,
            },
        },
    )?;
    stored.omitted_prefix_entries = stored
        .omitted_prefix_entries
        .checked_add(prior_omitted_prefix_entries)
        .ok_or(PersistenceError::OmissionCountOverflow)?;
    Ok(stored)
}

pub fn encode_stored(capture: &StoredCapture) -> Result<Vec<u8>, PersistenceError> {
    let disk = disk_bundle(
        &capture.bundle,
        capture.captured_unix_ms,
        capture.retention,
        capture.omitted_prefix_entries,
    );
    Ok(serde_json::to_vec_pretty(&disk)?)
}

fn disk_bundle(
    bundle: &ObservationBundle,
    captured_unix_ms: u64,
    retention: Retention,
    omitted_prefix_entries: usize,
) -> DiskBundle {
    DiskBundle {
        schema_version: DISK_SCHEMA_VERSION,
        bundle_version: bundle.version(),
        captured_unix_ms,
        retention: DiskRetention {
            max_entries: retention.max_entries,
            max_payload_bytes: retention.max_payload_bytes,
            max_age_ms: retention.max_age_ms,
        },
        omitted_prefix_entries,
        device_hex: hex::encode(bundle.device()),
        carrier: carrier_to_disk(bundle.carrier()),
        carrier_label: bundle.carrier_label().to_owned(),
        profiles: bundle
            .profiles()
            .iter()
            .map(|profile| DiskProfile {
                id: profile.id,
                version: profile.version,
                name: profile.name.clone(),
                definition_hex: hex::encode(&profile.definition),
            })
            .collect(),
        entries: bundle
            .entries()
            .iter()
            .map(|entry| match entry {
                BundleEntry::Record(record) => DiskEntry::Record {
                    received_unix_ms: record.received_unix_ms,
                    raw_hex: hex::encode(&record.raw),
                },
                BundleEntry::Disconnected { received_unix_ms } => DiskEntry::Disconnected {
                    received_unix_ms: *received_unix_ms,
                },
            })
            .collect(),
    }
}

pub fn decode(bytes: &[u8], limits: ReadLimits) -> Result<StoredCapture, PersistenceError> {
    if bytes.len() > limits.max_file_bytes {
        return Err(PersistenceError::FileTooLarge);
    }
    let disk: DiskBundle = serde_json::from_slice(bytes)?;
    if disk.schema_version != DISK_SCHEMA_VERSION || disk.bundle_version != BUNDLE_VERSION {
        return Err(PersistenceError::UnsupportedSchema);
    }
    if disk.retention.max_entries == 0
        || disk.retention.max_payload_bytes == 0
        || disk.entries.len() > disk.retention.max_entries
    {
        return Err(PersistenceError::RetentionTooSmall);
    }
    if let Some(max_age_ms) = disk.retention.max_age_ms {
        let cutoff = disk.captured_unix_ms.saturating_sub(max_age_ms);
        if disk.entries.iter().any(|entry| match entry {
            DiskEntry::Record {
                received_unix_ms, ..
            }
            | DiskEntry::Disconnected { received_unix_ms } => *received_unix_ms < cutoff,
        }) {
            return Err(PersistenceError::RetentionTooSmall);
        }
    }
    let device = hex::decode(disk.device_hex).map_err(|_| PersistenceError::InvalidHex)?;
    let profiles = disk
        .profiles
        .into_iter()
        .map(|profile| {
            Ok(ProfileEntry {
                id: profile.id,
                version: profile.version,
                name: profile.name,
                definition: hex::decode(profile.definition_hex)
                    .map_err(|_| PersistenceError::InvalidHex)?,
            })
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?;
    let retention = Retention {
        max_entries: disk.retention.max_entries,
        max_payload_bytes: disk.retention.max_payload_bytes,
        max_age_ms: disk.retention.max_age_ms,
    };
    let effective_admission = Admission {
        max_frames: limits.admission.max_frames.min(retention.max_entries),
        max_bytes: limits.admission.max_bytes.min(retention.max_payload_bytes),
    };
    let mut bundle = ObservationBundle::new(
        disk.bundle_version,
        &device,
        carrier_from_disk(disk.carrier),
        &disk.carrier_label,
        &profiles,
        effective_admission,
    )?;
    for entry in disk.entries {
        match entry {
            DiskEntry::Record {
                received_unix_ms,
                raw_hex,
            } => {
                let raw = hex::decode(raw_hex).map_err(|_| PersistenceError::InvalidHex)?;
                bundle.admit(&raw, received_unix_ms)?;
            }
            DiskEntry::Disconnected { received_unix_ms } => bundle.disconnect(received_unix_ms)?,
        }
    }
    Ok(StoredCapture {
        captured_unix_ms: disk.captured_unix_ms,
        retention,
        omitted_prefix_entries: disk.omitted_prefix_entries,
        bundle,
    })
}

pub fn read(path: &Path, limits: ReadLimits) -> Result<StoredCapture, PersistenceError> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take((limits.max_file_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    decode(&bytes, limits)
}

pub fn write_stored(path: &Path, capture: &StoredCapture) -> Result<(), PersistenceError> {
    publish_new(path, &encode_stored(capture)?)
}

pub fn store(
    destination: &DurableCapture,
    bundle: &ObservationBundle,
    now_unix_ms: u64,
    retention: Retention,
) -> Result<StoreOutcome, PersistenceError> {
    let DurableCapture::CreateNew(path) = destination else {
        return Ok(StoreOutcome::Disabled);
    };
    let bytes = encode(bundle, now_unix_ms, retention)?;
    publish_new(path, &bytes)?;
    Ok(StoreOutcome::Written {
        bytes: bytes.len(),
        entries: retained_entries(bundle, now_unix_ms, retention)?.len(),
    })
}

fn publish_new(path: &Path, bytes: &[u8]) -> Result<(), PersistenceError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("capture");
    let mut temporary = None;
    for attempt in 0..32_u8 {
        let candidate = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary_path, mut file) = temporary.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no temporary capture name available",
        )
    })?;
    let publish = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temporary_path, path)?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary_path);
    publish?;
    Ok(())
}
