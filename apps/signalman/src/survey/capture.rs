//! Capture import from third-party JSON and from Signalman's stored captures.

use sha2::{Digest, Sha256};

use super::{
    CAPTURE_SCHEMA_VERSION, CaptureIssue, CaptureLimits, CaptureRecord, CaptureSourceFormat,
    Direction, ImportError, ImportedCapture, ReverseEvidence, SourceHash,
};

fn json_string(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    map.get(key)?.as_str().map(ToOwned::to_owned)
}
fn json_u64(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    map.get(key)?.as_u64()
}
fn optional_string(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, ImportError> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or(ImportError::MalformedXml),
    }
}
fn optional_u64(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u64>, ImportError> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(ImportError::MalformedXml),
    }
}
fn json_finite(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<f64>, ImportError> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or(ImportError::InvalidCoordinate)
            .map(Some),
    }
}

/// Imports `signalman-survey-capture` schema 1. Unknown fields survive
/// import/export instead of being converted into made-up radio facts.
pub fn import_capture_json(
    bytes: &[u8],
    limits: CaptureLimits,
) -> Result<ImportedCapture, ImportError> {
    if bytes.len() > limits.max_bytes {
        return Err(ImportError::TooLarge);
    }
    let hash = SourceHash(Sha256::digest(bytes).into());
    let mut root: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|_| ImportError::MalformedXml)?;
    if root
        .remove("format")
        .and_then(|v| v.as_str().map(str::to_owned))
        .as_deref()
        != Some("signalman-survey-capture")
        || root.remove("schema_version").and_then(|v| v.as_u64()) != Some(1)
    {
        return Err(ImportError::InvalidTime);
    }
    let collector = match root.remove("collector") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(value.as_str().ok_or(ImportError::MalformedXml)?.to_owned()),
    };
    let raw = root
        .remove("records")
        .and_then(|v| v.as_array().cloned())
        .ok_or(ImportError::MissingCoordinate)?;
    if raw.len() > limits.max_records {
        return Err(ImportError::TooManyTrackPoints);
    }
    let mut records = Vec::with_capacity(raw.len());
    for value in raw {
        let mut map = value
            .as_object()
            .cloned()
            .ok_or(ImportError::MissingCoordinate)?;
        let observer = json_string(&map, "observer")
            .filter(|s| !s.is_empty())
            .ok_or(ImportError::MissingCoordinate)?;
        let direction = match json_string(&map, "direction").as_deref() {
            Some("received") => Direction::Received,
            Some("transmitted") => Direction::Transmitted,
            Some("test-opportunity") => Direction::TestOpportunity,
            _ => return Err(ImportError::InvalidTime),
        };
        let host_received_unix_ms =
            json_u64(&map, "host_received_unix_ms").ok_or(ImportError::InvalidTime)?;
        let peer = optional_string(&map, "peer")?;
        let boot_id = optional_u64(&map, "boot_id")?;
        let session = optional_string(&map, "session")?;
        let source_uptime_ms = optional_u64(&map, "source_uptime_ms")?;
        let sequence = optional_u64(&map, "sequence")?;
        let rssi_dbm = json_finite(&map, "rssi_dbm")?;
        let snr_db = json_finite(&map, "snr_db")?;
        for key in [
            "observer",
            "direction",
            "host_received_unix_ms",
            "peer",
            "boot_id",
            "session",
            "source_uptime_ms",
            "sequence",
            "rssi_dbm",
            "snr_db",
        ] {
            map.remove(key);
        }
        if map.len() > limits.max_unknown_fields {
            return Err(ImportError::TooDeep);
        }
        records.push(CaptureRecord {
            observer,
            peer,
            direction,
            boot_id,
            session,
            source_uptime_ms,
            sequence,
            host_received_unix_ms,
            rssi_dbm,
            snr_db,
            unknown: map,
        });
    }
    let issues = match root.remove("issues") {
        None => Vec::new(),
        Some(value) => serde_json::from_value(value).map_err(|_| ImportError::MalformedXml)?,
    };
    if issues.len() > limits.max_records {
        return Err(ImportError::TooManyTrackPoints);
    }
    if root.len() > limits.max_unknown_fields {
        return Err(ImportError::TooDeep);
    }
    Ok(ImportedCapture {
        schema_version: CAPTURE_SCHEMA_VERSION,
        source_format: CaptureSourceFormat::ThirdPartyJson,
        source_hash: hash,
        source_bytes: bytes.to_vec(),
        collector,
        records,
        issues,
        unknown: root,
    })
}

pub fn reverse_evidence(capture: &ImportedCapture, observer: &str, peer: &str) -> ReverseEvidence {
    for record in &capture.records {
        if record.observer == peer && record.peer.as_deref() == Some(observer) {
            match record.direction {
                Direction::Received => return ReverseEvidence::Observed,
                // A transmission opportunity does not certify the listening
                // window, profile, clock coverage, or absence of capture loss.
                Direction::TestOpportunity => {}
                Direction::Transmitted => {}
            }
        }
    }
    ReverseEvidence::Unknown
}

/// Adapter for Signalman's immutable schema-1 saved capture. It preserves
/// source boot/uptime/sequence separately from `received_unix_ms`; raw gaps
/// and disconnects become explicit unknown intervals. Current radio records
/// do not carry a remote peer identity, so this adapter never fabricates one.
fn derived_capture_from_stored_capture(
    capture: &crate::observation::persistence::StoredCapture,
    observer: &str,
) -> ImportedCapture {
    use crate::observation::BundleEntry;
    use radio_hand::observation::{ObservationKind, ObservationRecord};
    let mut records = Vec::new();
    let mut issues = Vec::new();
    for entry in capture.bundle.entries() {
        match entry {
            BundleEntry::Disconnected { .. } => {
                issues.push(CaptureIssue::CollectorDisconnected);
            }
            BundleEntry::Record(frame) => match ObservationRecord::decode(&frame.raw) {
                Ok(ObservationRecord::Gap(gap)) => issues.push(CaptureIssue::Gap {
                    boot_id: gap.boot_id,
                    first_missing: gap.first_missing,
                    count: gap.count,
                }),
                Ok(ObservationRecord::Event(event)) => {
                    let (direction, rssi_dbm, snr_db) = match event.kind {
                        ObservationKind::RxCaptured {
                            rssi_dbm,
                            snr_tenths_db,
                            ..
                        } => (
                            Direction::Received,
                            Some(f64::from(rssi_dbm)),
                            Some(f64::from(snr_tenths_db) / 10.0),
                        ),
                        ObservationKind::TxStarted { .. } => (Direction::Transmitted, None, None),
                        _ => continue,
                    };
                    records.push(CaptureRecord {
                        observer: observer.into(),
                        peer: None,
                        direction,
                        boot_id: Some(event.boot_id),
                        session: None,
                        source_uptime_ms: Some(event.uptime_ms),
                        sequence: Some(event.sequence),
                        host_received_unix_ms: frame.received_unix_ms,
                        rssi_dbm,
                        snr_db,
                        unknown: serde_json::Map::new(),
                    });
                }
                Err(_) => issues.push(CaptureIssue::CaptureLoss {
                    detail: "stored record no longer decodes".into(),
                }),
            },
        }
    }
    ImportedCapture {
        schema_version: CAPTURE_SCHEMA_VERSION,
        source_format: CaptureSourceFormat::StoredCaptureSchema1 {
            observer: observer.into(),
        },
        source_hash: SourceHash([0; 32]),
        source_bytes: Vec::new(),
        collector: Some("signalman-stored-capture-schema-1".into()),
        records,
        issues,
        unknown: serde_json::Map::new(),
    }
}

/// Decode a bounded original StoredCapture file, and retain its full JSON
/// envelope as opaque evidence. The digest is always of the caller's original
/// bytes, never a reconstructed subset of entries.
pub fn import_stored_capture_json(
    bytes: &[u8],
    limits: crate::observation::persistence::ReadLimits,
    observer: &str,
) -> Result<ImportedCapture, ImportError> {
    if observer.is_empty() {
        return Err(ImportError::MissingCoordinate);
    }
    if bytes.len() > limits.max_file_bytes {
        return Err(ImportError::TooLarge);
    }
    let capture = crate::observation::persistence::decode(bytes, limits)
        .map_err(|_| ImportError::MalformedXml)?;
    let mut imported = derived_capture_from_stored_capture(&capture, observer);
    imported.source_hash = SourceHash(Sha256::digest(bytes).into());
    imported.source_bytes = bytes.to_vec();
    let envelope = serde_json::from_slice(bytes).map_err(|_| ImportError::MalformedXml)?;
    imported
        .unknown
        .insert("stored_capture_schema_1".into(), envelope);
    Ok(imported)
}
