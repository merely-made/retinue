//! Bounded import of owner-supplied survey tracks.
//!
//! Imported positions are source evidence. They do not authenticate a board,
//! identify a radio event, or turn a host receipt time into RF time.

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportLimits {
    pub max_bytes: usize,
    pub max_track_points: usize,
    pub max_xml_depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    TooLarge,
    TooManyTrackPoints,
    TooDeep,
    Doctype,
    MalformedXml,
    MissingCoordinate,
    InvalidCoordinate,
    InvalidElevation,
    InvalidTime,
    EmptyTrackPoint,
    NestedTrackPoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackPoint {
    /// Zero-based source `trkseg` identity. Coordinates are GPX WGS 84
    /// latitude/longitude; `elevation_m` is GPX elevation in metres.
    pub track_segment: usize,
    pub latitude: f64,
    pub longitude: f64,
    pub elevation_m: Option<f64>,
    /// The original UTC GPX text after structural validation. A later clock
    /// join may map it to host time; it must retain this source value.
    pub source_time: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHash(pub [u8; 32]);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedTrack {
    pub schema_version: u8,
    pub source_hash: SourceHash,
    /// Exact bounded GPX bytes retained as immutable source evidence.
    pub source_bytes: Vec<u8>,
    pub points: Vec<TrackPoint>,
}

impl ImportedTrack {
    pub const SCHEMA_VERSION: u8 = 1;
}

/// Versioned, deliberately small JSON interchange for captures produced by
/// tools outside Retinue. The `unknown` maps preserve unrecognised evidence.
pub const CAPTURE_SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureLimits {
    pub max_bytes: usize,
    pub max_records: usize,
    pub max_unknown_fields: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Received,
    Transmitted,
    TestOpportunity,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub observer: String,
    pub peer: Option<String>,
    pub direction: Direction,
    pub boot_id: Option<u64>,
    pub session: Option<String>,
    pub source_uptime_ms: Option<u64>,
    pub sequence: Option<u64>,
    /// Collection provenance only. It is never used as radio-event time.
    pub host_received_unix_ms: u64,
    pub rssi_dbm: Option<f64>,
    pub snr_db: Option<f64>,
    #[serde(default)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CaptureIssue {
    Reset {
        boot_id: u64,
    },
    Gap {
        boot_id: u64,
        first_missing: u64,
        count: u64,
    },
    CaptureLoss {
        detail: String,
    },
    CollectorDisconnected,
    Unknown {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedCapture {
    pub schema_version: u8,
    pub source_format: CaptureSourceFormat,
    pub source_hash: SourceHash,
    /// Exact bounded source bytes. Provenance, not an authentication claim.
    pub source_bytes: Vec<u8>,
    pub collector: Option<String>,
    pub records: Vec<CaptureRecord>,
    pub issues: Vec<CaptureIssue>,
    #[serde(default)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CaptureSourceFormat {
    ThirdPartyJson,
    StoredCaptureSchema1 { observer: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockMapping {
    pub observer: String,
    pub boot_id: Option<u64>,
    pub session: Option<String>,
    pub source_anchor_ms: u64,
    pub unix_anchor_ms: u64,
    pub drift_ppm: i32,
    pub uncertainty_ms: u64,
}

impl ClockMapping {
    /// Returns civil time and its configured uncertainty only for the same
    /// observer and boot/session. A host receipt is intentionally excluded.
    pub fn map(&self, record: &CaptureRecord) -> Option<(u64, u64)> {
        if self.boot_id.is_none() && self.session.is_none()
            || self.observer != record.observer
            || self.boot_id != record.boot_id
            || self.session != record.session
        {
            return None;
        }
        let delta =
            i128::from(record.source_uptime_ms?).checked_sub(i128::from(self.source_anchor_ms))?;
        let adjusted = i128::from(self.unix_anchor_ms)
            .checked_add(delta)?
            .checked_add(
                delta
                    .checked_mul(i128::from(self.drift_ppm))?
                    .checked_div(1_000_000)?,
            )?;
        Some((u64::try_from(adjusted).ok()?, self.uncertainty_ms))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisSettings {
    pub clock_mappings: Vec<ClockMapping>,
    pub interpolation_gap_ms: u64,
    pub freshness_window_ms: u64,
    pub spatial_bucket_m: u32,
    pub selected_sources: Vec<SourceHash>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Survey {
    pub schema_version: u8,
    pub tracks: Vec<ImportedTrack>,
    pub captures: Vec<ImportedCapture>,
    pub settings: AnalysisSettings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReverseEvidence {
    Observed,
    VerifiedNoReception,
    Unknown,
}

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

pub fn export_survey(survey: &Survey) -> Result<Vec<u8>, ImportError> {
    if survey.schema_version != 1 {
        return Err(ImportError::InvalidTime);
    }
    serde_json::to_vec_pretty(survey).map_err(|_| ImportError::MalformedXml)
}
pub fn import_survey(bytes: &[u8], max_bytes: usize) -> Result<Survey, ImportError> {
    if bytes.len() > max_bytes {
        return Err(ImportError::TooLarge);
    }
    let survey: Survey = serde_json::from_slice(bytes).map_err(|_| ImportError::MalformedXml)?;
    if survey.schema_version != 1 {
        return Err(ImportError::InvalidTime);
    }
    validate_survey(&survey)?;
    Ok(survey)
}

/// Reimport is an untrusted boundary even when bytes came from our own export.
/// Keep its admission checks independent of serde's structural decoding.
fn validate_survey(survey: &Survey) -> Result<(), ImportError> {
    const MAX_TRACKS: usize = 256;
    const MAX_CAPTURES: usize = 256;
    const MAX_POINTS: usize = 100_000;
    const MAX_RECORDS: usize = 100_000;
    const MAX_UNKNOWN: usize = 256;
    if survey.tracks.len() > MAX_TRACKS || survey.captures.len() > MAX_CAPTURES {
        return Err(ImportError::TooManyTrackPoints);
    }
    for track in &survey.tracks {
        if track.schema_version != ImportedTrack::SCHEMA_VERSION || track.points.len() > MAX_POINTS
        {
            return Err(ImportError::TooManyTrackPoints);
        }
        if track.source_bytes.is_empty()
            || SourceHash(Sha256::digest(&track.source_bytes).into()) != track.source_hash
        {
            return Err(ImportError::MalformedXml);
        }
        let rebuilt = import_gpx(
            &track.source_bytes,
            ImportLimits {
                max_bytes: track.source_bytes.len(),
                max_track_points: MAX_POINTS,
                max_xml_depth: 256,
            },
        )?;
        if &rebuilt != track {
            return Err(ImportError::MalformedXml);
        }
        for point in &track.points {
            if !point.latitude.is_finite()
                || point.latitude.abs() > 90.0
                || !point.longitude.is_finite()
                || point.longitude.abs() > 180.0
            {
                return Err(ImportError::InvalidCoordinate);
            }
            if point.elevation_m.is_some_and(|value| !value.is_finite()) {
                return Err(ImportError::InvalidElevation);
            }
            if point
                .source_time
                .as_deref()
                .is_some_and(|value| !valid_utc_time(value))
            {
                return Err(ImportError::InvalidTime);
            }
        }
    }
    for capture in &survey.captures {
        if capture.schema_version != CAPTURE_SCHEMA_VERSION
            || capture.records.len() > MAX_RECORDS
            || capture.unknown.len() > MAX_UNKNOWN
        {
            return Err(ImportError::TooManyTrackPoints);
        }
        if capture.source_bytes.is_empty()
            || capture.source_bytes.len() > MAX_RECORDS.saturating_mul(1024)
            || SourceHash(Sha256::digest(&capture.source_bytes).into()) != capture.source_hash
        {
            return Err(ImportError::MalformedXml);
        }
        for record in &capture.records {
            if record.observer.is_empty()
                || record.unknown.len() > MAX_UNKNOWN
                || record.rssi_dbm.is_some_and(|value| !value.is_finite())
                || record.snr_db.is_some_and(|value| !value.is_finite())
            {
                return Err(ImportError::MalformedXml);
            }
            if record.boot_id.is_none()
                && record.session.is_none()
                && record.source_uptime_ms.is_some()
            {
                return Err(ImportError::InvalidTime);
            }
        }
        for issue in &capture.issues {
            if let CaptureIssue::Gap { count, .. } = issue
                && *count == 0
            {
                return Err(ImportError::InvalidTime);
            }
        }
        if let CaptureSourceFormat::StoredCaptureSchema1 { observer } = &capture.source_format {
            let limits = crate::observation::persistence::ReadLimits {
                max_file_bytes: capture.source_bytes.len(),
                admission: crate::observation::Admission {
                    max_frames: MAX_RECORDS,
                    max_bytes: capture.source_bytes.len(),
                },
            };
            let rebuilt = import_stored_capture_json(&capture.source_bytes, limits, observer)?;
            if &rebuilt != capture {
                return Err(ImportError::MalformedXml);
            }
        } else {
            let rebuilt = import_capture_json(
                &capture.source_bytes,
                CaptureLimits {
                    max_bytes: capture.source_bytes.len(),
                    max_records: MAX_RECORDS,
                    max_unknown_fields: MAX_UNKNOWN,
                },
            )?;
            if &rebuilt != capture {
                return Err(ImportError::MalformedXml);
            }
        }
    }
    if survey.settings.clock_mappings.len() > MAX_RECORDS
        || survey.settings.selected_sources.len() > MAX_CAPTURES
    {
        return Err(ImportError::TooManyTrackPoints);
    }
    for mapping in &survey.settings.clock_mappings {
        if mapping.observer.is_empty() || mapping.boot_id.is_none() && mapping.session.is_none() {
            return Err(ImportError::InvalidTime);
        }
    }
    Ok(())
}

fn parse_coordinate(value: Option<&[u8]>, latitude: bool) -> Result<f64, ImportError> {
    let value = value.ok_or(ImportError::MissingCoordinate)?;
    let value = std::str::from_utf8(value).map_err(|_| ImportError::InvalidCoordinate)?;
    let number = value
        .parse::<f64>()
        .map_err(|_| ImportError::InvalidCoordinate)?;
    let limit = if latitude { 90.0 } else { 180.0 };
    if !number.is_finite() || number.abs() > limit {
        return Err(ImportError::InvalidCoordinate);
    }
    Ok(number)
}

fn prefixed_gpx_core(name: &[u8]) -> bool {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return false;
    };
    let local = &name[separator + 1..];
    matches!(
        local,
        b"gpx" | b"trk" | b"trkseg" | b"trkpt" | b"ele" | b"time"
    )
}

fn valid_gpx_parent(name: &[u8], parent: Option<&[u8]>) -> bool {
    match name {
        b"gpx" => parent.is_none(),
        b"trk" => parent == Some(b"gpx"),
        b"trkseg" => parent == Some(b"trk"),
        b"trkpt" => parent == Some(b"trkseg"),
        b"ele" | b"time" => parent == Some(b"trkpt"),
        _ => true,
    }
}

fn valid_utc_time(value: &str) -> bool {
    // GPX timestamps are RFC 3339 UTC. Keep the literal source value, but do
    // reject empty, non-UTC, and impossible-shape inputs before persistence.
    fn number(bytes: &[u8]) -> Option<u32> {
        std::str::from_utf8(bytes).ok()?.parse().ok()
    }

    let bytes = value.as_bytes();
    if !(bytes.len() >= 20
        && bytes.ends_with(b"Z")
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && number(&bytes[0..4]).is_some()
        && number(&bytes[5..7]).is_some_and(|month| (1..=12).contains(&month))
        && number(&bytes[8..10]).is_some_and(|day| (1..=31).contains(&day))
        && number(&bytes[11..13]).is_some_and(|hour| hour <= 23)
        && number(&bytes[14..16]).is_some_and(|minute| minute <= 59)
        && number(&bytes[17..19]).is_some_and(|second| second <= 60))
    {
        return false;
    }
    let year = number(&bytes[0..4]).unwrap();
    let month = number(&bytes[5..7]).unwrap();
    let day = number(&bytes[8..10]).unwrap();
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if day > days {
        return false;
    }
    let fraction = &bytes[19..bytes.len() - 1];
    fraction.is_empty()
        || (fraction[0] == b'.'
            && fraction.len() > 1
            && fraction[1..].iter().all(|byte| byte.is_ascii_digit()))
}

/// Read GPX without loading external entities or resources.
pub fn import_gpx(bytes: &[u8], limits: ImportLimits) -> Result<ImportedTrack, ImportError> {
    if bytes.len() > limits.max_bytes {
        return Err(ImportError::TooLarge);
    }
    let source_hash = SourceHash(Sha256::digest(bytes).into());
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut points = Vec::new();
    let mut current: Option<TrackPoint> = None;
    let mut current_point_depth = None;
    let mut current_segment = None;
    let mut next_segment = 0_usize;
    let mut field: Option<&'static str> = None;
    let mut seen_elevation = false;
    let mut seen_time = false;
    let mut scalar_value_seen = false;
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut saw_root = false;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::DocType(_)) => return Err(ImportError::Doctype),
            Ok(Event::Start(element)) => {
                let name = element.name().as_ref().to_vec();
                if name.as_slice() == b"trkpt" && current.is_some() {
                    return Err(ImportError::NestedTrackPoint);
                }
                if prefixed_gpx_core(&name)
                    || field.is_some()
                    || !valid_gpx_parent(&name, stack.last().map(Vec::as_slice))
                    || (stack.is_empty() && name.as_slice() != b"gpx")
                    || (name.as_slice() == b"gpx" && saw_root)
                {
                    return Err(ImportError::MalformedXml);
                }
                if name.as_slice() == b"gpx" {
                    saw_root = true;
                }
                depth = depth.checked_add(1).ok_or(ImportError::TooDeep)?;
                if depth > limits.max_xml_depth {
                    return Err(ImportError::TooDeep);
                }
                match element.name().as_ref() {
                    b"trkseg" => {
                        if current_segment.is_some() || current.is_some() {
                            return Err(ImportError::MalformedXml);
                        }
                        current_segment = Some(next_segment);
                        next_segment = next_segment.checked_add(1).ok_or(ImportError::TooDeep)?;
                    }
                    b"trkpt" => {
                        if current.is_some() {
                            return Err(ImportError::NestedTrackPoint);
                        }
                        let track_segment = current_segment.ok_or(ImportError::MalformedXml)?;
                        if points.len() == limits.max_track_points {
                            return Err(ImportError::TooManyTrackPoints);
                        }
                        let mut latitude = None;
                        let mut longitude = None;
                        for attribute in element.attributes() {
                            let attribute = attribute.map_err(|_| ImportError::MalformedXml)?;
                            match attribute.key.as_ref() {
                                b"lat" => latitude = Some(attribute.value.as_ref().to_vec()),
                                b"lon" => longitude = Some(attribute.value.as_ref().to_vec()),
                                _ => {}
                            }
                        }
                        current = Some(TrackPoint {
                            track_segment,
                            latitude: parse_coordinate(latitude.as_deref(), true)?,
                            longitude: parse_coordinate(longitude.as_deref(), false)?,
                            elevation_m: None,
                            source_time: None,
                        });
                        current_point_depth = Some(depth);
                        seen_elevation = false;
                        seen_time = false;
                    }
                    b"ele" if current_point_depth == depth.checked_sub(1) => {
                        if seen_elevation {
                            return Err(ImportError::MalformedXml);
                        }
                        seen_elevation = true;
                        field = Some("ele");
                        scalar_value_seen = false;
                    }
                    b"time" if current_point_depth == depth.checked_sub(1) => {
                        if seen_time {
                            return Err(ImportError::MalformedXml);
                        }
                        seen_time = true;
                        field = Some("time");
                        scalar_value_seen = false;
                    }
                    _ => {}
                }
                stack.push(name);
            }
            Ok(Event::Text(text)) => {
                if let (Some(point), Some(field)) = (current.as_mut(), field) {
                    if scalar_value_seen {
                        return Err(ImportError::MalformedXml);
                    }
                    let value = std::str::from_utf8(text.as_ref())
                        .map_err(|_| ImportError::MalformedXml)?;
                    match field {
                        "ele" => {
                            let elevation = value
                                .parse::<f64>()
                                .map_err(|_| ImportError::InvalidElevation)?;
                            if !elevation.is_finite() {
                                return Err(ImportError::InvalidElevation);
                            }
                            point.elevation_m = Some(elevation);
                        }
                        "time" => {
                            if !valid_utc_time(value) {
                                return Err(ImportError::InvalidTime);
                            }
                            point.source_time = Some(value.to_owned());
                        }
                        _ => unreachable!(),
                    }
                    scalar_value_seen = true;
                }
            }
            Ok(Event::CData(_) | Event::GeneralRef(_)) if field.is_some() => {
                return Err(ImportError::MalformedXml);
            }
            Ok(Event::Empty(element)) => {
                let name = element.name().as_ref().to_vec();
                if name.as_slice() == b"trkpt" && current.is_some() {
                    return Err(ImportError::NestedTrackPoint);
                }
                if prefixed_gpx_core(&name)
                    || field.is_some()
                    || !valid_gpx_parent(&name, stack.last().map(Vec::as_slice))
                    || (stack.is_empty() && name.as_slice() != b"gpx")
                    || (name.as_slice() == b"gpx" && saw_root)
                {
                    return Err(ImportError::MalformedXml);
                }
                if name.as_slice() == b"gpx" {
                    saw_root = true;
                }
                if depth.checked_add(1).ok_or(ImportError::TooDeep)? > limits.max_xml_depth {
                    return Err(ImportError::TooDeep);
                }
                if element.name().as_ref() == b"trkpt" {
                    if current.is_some() {
                        return Err(ImportError::NestedTrackPoint);
                    }
                    let track_segment = current_segment.ok_or(ImportError::MalformedXml)?;
                    if points.len() == limits.max_track_points {
                        return Err(ImportError::TooManyTrackPoints);
                    }
                    let mut latitude = None;
                    let mut longitude = None;
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|_| ImportError::MalformedXml)?;
                        match attribute.key.as_ref() {
                            b"lat" => latitude = Some(attribute.value.as_ref().to_vec()),
                            b"lon" => longitude = Some(attribute.value.as_ref().to_vec()),
                            _ => {}
                        }
                    }
                    points.push(TrackPoint {
                        track_segment,
                        latitude: parse_coordinate(latitude.as_deref(), true)?,
                        longitude: parse_coordinate(longitude.as_deref(), false)?,
                        elevation_m: None,
                        source_time: None,
                    });
                } else if current_point_depth == Some(depth)
                    && matches!(element.name().as_ref(), b"ele" | b"time")
                {
                    return Err(ImportError::MalformedXml);
                }
            }
            Ok(Event::End(element)) => {
                if stack.pop().as_deref() != Some(element.name().as_ref()) {
                    return Err(ImportError::MalformedXml);
                }
                if element.name().as_ref() == b"trkpt" {
                    points.push(current.take().ok_or(ImportError::EmptyTrackPoint)?);
                    current_point_depth = None;
                }
                if element.name().as_ref() == b"trkseg"
                    && (current.is_some() || current_segment.take().is_none())
                {
                    return Err(ImportError::MalformedXml);
                }
                if matches!(element.name().as_ref(), b"ele" | b"time") {
                    if !scalar_value_seen {
                        return Err(ImportError::MalformedXml);
                    }
                    field = None;
                    scalar_value_seen = false;
                }
                depth = depth.checked_sub(1).ok_or(ImportError::MalformedXml)?;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(ImportError::MalformedXml),
        }
        buffer.clear();
    }
    if current.is_some() || depth != 0 || !stack.is_empty() || !saw_root {
        return Err(ImportError::MalformedXml);
    }
    Ok(ImportedTrack {
        schema_version: ImportedTrack::SCHEMA_VERSION,
        source_hash,
        source_bytes: bytes.to_vec(),
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ImportLimits {
        ImportLimits {
            max_bytes: 4_096,
            max_track_points: 8,
            max_xml_depth: 8,
        }
    }

    #[test]
    fn imports_a_moving_track_without_rewriting_source_times() {
        let source = br#"<gpx><trk><trkseg><trkpt lat="40.0" lon="-73.0"><ele>12.5</ele><time>2026-09-10T12:00:00Z</time></trkpt><trkpt lat="40.1" lon="-73.1" /></trkseg></trk></gpx>"#;
        let track = import_gpx(source, limits()).unwrap();
        assert_eq!(track.points.len(), 2);
        assert_eq!(
            track.points[0].source_time.as_deref(),
            Some("2026-09-10T12:00:00Z")
        );
        assert_ne!(track.source_hash, SourceHash([0; 32]));
        assert_eq!(track.source_bytes, source);
        assert_eq!(import_gpx(source, limits()).unwrap(), track);
    }

    #[test]
    fn imports_stationary_points_and_retains_a_long_gps_gap() {
        let source = br#"<gpx><trk><trkseg><trkpt lat="40" lon="-73"><time>2026-09-10T12:00:00Z</time></trkpt><trkpt lat="40" lon="-73"><time>2026-09-10T12:30:00Z</time></trkpt></trkseg></trk></gpx>"#;
        let track = import_gpx(source, limits()).unwrap();
        assert_eq!(track.points[0].latitude, track.points[1].latitude);
        assert_eq!(track.points[0].longitude, track.points[1].longitude);
        assert_eq!(
            track.points[0].source_time.as_deref(),
            Some("2026-09-10T12:00:00Z")
        );
        assert_eq!(
            track.points[1].source_time.as_deref(),
            Some("2026-09-10T12:30:00Z")
        );
    }

    #[test]
    fn refuses_external_entity_declarations_and_nonfinite_coordinates() {
        assert_eq!(
            import_gpx(br#"<!DOCTYPE gpx [<!ENTITY x "x">]><gpx/>"#, limits()),
            Err(ImportError::Doctype)
        );
        assert_eq!(
            import_gpx(
                br#"<gpx><trk><trkseg><trkpt lat="NaN" lon="0"/></trkseg></trk></gpx>"#,
                limits()
            ),
            Err(ImportError::InvalidCoordinate)
        );
    }

    fn capture_limits() -> CaptureLimits {
        CaptureLimits {
            max_bytes: 4_096,
            max_records: 8,
            max_unknown_fields: 4,
        }
    }

    #[test]
    fn rejects_impossible_dates_bad_fractions_and_nested_points() {
        assert_eq!(
            import_gpx(
                br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><time>2026-02-31T00:00:00Z</time></trkpt></trkseg></trk></gpx>"#,
                limits()
            ),
            Err(ImportError::InvalidTime)
        );
        assert_eq!(import_gpx(br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><time>2026-02-01T00:00:00.1.2Z</time></trkpt></trkseg></trk></gpx>"#, limits()), Err(ImportError::InvalidTime));
        assert_eq!(
            import_gpx(
                br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><trkpt lat="1" lon="1"/></trkpt></trkseg></trk></gpx>"#,
                limits()
            ),
            Err(ImportError::NestedTrackPoint)
        );
        let segmented = import_gpx(
            br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"/></trkseg><trkseg><trkpt lat="1" lon="1"/></trkseg></trk></gpx>"#,
            limits(),
        )
        .unwrap();
        assert_eq!(segmented.points[0].track_segment, 0);
        assert_eq!(segmented.points[1].track_segment, 1);
        for malformed in [
            br#"<p:gpx xmlns:p="urn:gpx"><p:trk><p:trkseg/></p:trk></p:gpx>"#.as_slice(),
            br#"<gpx><trkseg><trkpt lat="0" lon="0"/></trkseg></gpx>"#.as_slice(),
            br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><extensions><time>2026-02-01T00:00:00Z</time></extensions></trkpt></trkseg></trk></gpx>"#.as_slice(),
            br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><time><x>2026-02-01T00:00:00Z</x></time></trkpt></trkseg></trk></gpx>"#.as_slice(),
            br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><ele>1</ele><ele>2</ele></trkpt></trkseg></trk></gpx>"#.as_slice(),
            br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><ele></ele></trkpt></trkseg></trk></gpx>"#.as_slice(),
            br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><ele><![CDATA[1]]></ele></trkpt></trkseg></trk></gpx>"#.as_slice(),
            br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><ele>1<!--split-->2</ele></trkpt></trkseg></trk></gpx>"#.as_slice(),
        ] {
            assert_eq!(import_gpx(malformed, limits()), Err(ImportError::MalformedXml));
        }
        assert_eq!(
            import_gpx(
                br#"<gpx><trk><extension/></trk></gpx>"#,
                ImportLimits {
                    max_xml_depth: 2,
                    ..limits()
                }
            ),
            Err(ImportError::TooDeep)
        );
    }

    #[test]
    fn capture_import_has_explicit_clock_and_unknown_reverse_evidence() {
        let bytes = br#"{"format":"signalman-survey-capture","schema_version":1,"records":[{"observer":"a","peer":"b","direction":"received","boot_id":7,"source_uptime_ms":2000,"sequence":2,"host_received_unix_ms":999,"vendor":"kept"}],"future":true}"#;
        let capture = import_capture_json(bytes, capture_limits()).unwrap();
        assert!(capture.unknown.contains_key("future"));
        assert!(capture.records[0].unknown.contains_key("vendor"));
        assert_eq!(
            reverse_evidence(&capture, "a", "b"),
            ReverseEvidence::Unknown
        );
        let mapping = ClockMapping {
            observer: "a".into(),
            boot_id: Some(7),
            session: None,
            source_anchor_ms: 1000,
            unix_anchor_ms: 10_000,
            drift_ppm: 0,
            uncertainty_ms: 12,
        };
        assert_eq!(mapping.map(&capture.records[0]), Some((11_000, 12)));
        let reset_mapping = ClockMapping {
            boot_id: Some(8),
            ..mapping
        };
        assert_eq!(reset_mapping.map(&capture.records[0]), None);
        let unscoped_mapping = ClockMapping {
            boot_id: None,
            session: None,
            ..reset_mapping
        };
        assert_eq!(unscoped_mapping.map(&capture.records[0]), None);
    }

    #[test]
    fn imports_reset_capture_loss_and_asymmetric_reception_as_distinct_evidence() {
        let bytes = br#"{"format":"signalman-survey-capture","schema_version":1,"records":[{"observer":"a","peer":"b","direction":"received","boot_id":7,"source_uptime_ms":10,"host_received_unix_ms":100}],"issues":[{"kind":"reset","boot_id":8},{"kind":"capture-loss","detail":"buffer overflow"}]}"#;
        let capture = import_capture_json(bytes, capture_limits()).unwrap();
        assert_eq!(
            capture.issues,
            vec![
                CaptureIssue::Reset { boot_id: 8 },
                CaptureIssue::CaptureLoss {
                    detail: "buffer overflow".into()
                },
            ]
        );
        assert_eq!(
            reverse_evidence(&capture, "a", "b"),
            ReverseEvidence::Unknown
        );
    }

    #[test]
    fn refuses_oversize_nonfinite_and_wrong_typed_optional_capture_fields() {
        assert_eq!(
            import_capture_json(
                b"{}",
                CaptureLimits {
                    max_bytes: 1,
                    ..capture_limits()
                }
            ),
            Err(ImportError::TooLarge)
        );
        for bytes in [
            br#"{"format":"signalman-survey-capture","schema_version":1,"records":[{"observer":"a","direction":"received","host_received_unix_ms":1,"rssi_dbm":1e999}]}"#.as_slice(),
            br#"{"format":"signalman-survey-capture","schema_version":1,"records":[{"observer":"a","peer":7,"direction":"received","host_received_unix_ms":1}]}"#.as_slice(),
            br#"{"format":"signalman-survey-capture","schema_version":1,"collector":7,"records":[]}"#.as_slice(),
        ] {
            assert!(import_capture_json(bytes, capture_limits()).is_err());
        }
    }

    #[test]
    fn reverse_requires_receipt_or_test_opportunity_and_export_replays_settings() {
        let bytes = br#"{"format":"signalman-survey-capture","schema_version":1,"records":[{"observer":"b","peer":"a","direction":"test-opportunity","host_received_unix_ms":1}],"stored_capture_schema_1":"third-party-unknown"}"#;
        let capture = import_capture_json(bytes, capture_limits()).unwrap();
        assert_eq!(
            reverse_evidence(&capture, "a", "b"),
            ReverseEvidence::Unknown
        );
        let survey = Survey {
            schema_version: 1,
            tracks: vec![],
            captures: vec![capture],
            settings: AnalysisSettings {
                clock_mappings: vec![],
                interpolation_gap_ms: 1_000,
                freshness_window_ms: 2_000,
                spatial_bucket_m: 25,
                selected_sources: vec![],
            },
        };
        assert_eq!(
            import_survey(&export_survey(&survey).unwrap(), 4_096).unwrap(),
            survey
        );
    }

    #[test]
    fn reimport_rejects_a_summary_that_disagrees_with_original_source_bytes() {
        let bytes = br#"{"format":"signalman-survey-capture","schema_version":1,"records":[{"observer":"a","peer":"b","direction":"received","host_received_unix_ms":1}]}"#;
        let capture = import_capture_json(bytes, capture_limits()).unwrap();
        let mut survey = Survey {
            schema_version: 1,
            tracks: vec![],
            captures: vec![capture],
            settings: AnalysisSettings {
                clock_mappings: vec![],
                interpolation_gap_ms: 1_000,
                freshness_window_ms: 2_000,
                spatial_bucket_m: 25,
                selected_sources: vec![],
            },
        };
        survey.captures[0].records[0].observer = "tampered".into();
        let encoded = export_survey(&survey).unwrap();
        assert_eq!(
            import_survey(&encoded, encoded.len()),
            Err(ImportError::MalformedXml)
        );
    }

    #[test]
    fn reimport_revalidates_nonfinite_coordinates_and_clock_scope() {
        let bad_coordinate = br#"{"schema_version":1,"tracks":[{"schema_version":1,"source_hash":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"points":[{"latitude":1e999,"longitude":0,"elevation_m":null,"source_time":null}]}],"captures":[],"settings":{"clock_mappings":[],"interpolation_gap_ms":1,"freshness_window_ms":1,"spatial_bucket_m":1,"selected_sources":[]}}"#;
        assert!(import_survey(bad_coordinate, 4_096).is_err());
        let bad_mapping = br#"{"schema_version":1,"tracks":[],"captures":[],"settings":{"clock_mappings":[{"observer":"a","boot_id":null,"session":null,"source_anchor_ms":0,"unix_anchor_ms":0,"drift_ppm":0,"uncertainty_ms":0}],"interpolation_gap_ms":1,"freshness_window_ms":1,"spatial_bucket_m":1,"selected_sources":[]}}"#;
        assert!(import_survey(bad_mapping, 4_096).is_err());
    }

    #[test]
    fn stored_capture_adapter_preserves_literal_evidence_and_rejects_tampered_summary() {
        let literal = br#"{
          "schema_version":1,"bundle_version":1,"captured_unix_ms":2000,
          "retention":{"max_entries":8,"max_payload_bytes":4096,"max_age_ms":null},
          "omitted_prefix_entries":0,"device_hex":"62656e63682d626f617264",
          "carrier":{"kind":"local-usb"},"carrier_label":"fixture",
          "profiles":[{"id":3,"version":1,"name":"fixture","definition_hex":"66697874757265"}],
          "entries":[
            {"kind":"record","received_unix_ms":1100,"raw_hex":"4f0100002d00000000000000010000000000000001000000000000000202030004ffba007d0000000931e80871"},
            {"kind":"record","received_unix_ms":1110,"raw_hex":"4f01010021000000000000000200000000000000080000000000000004334af9a6"},
            {"kind":"disconnected","received_unix_ms":1120}
          ]
        }"#;
        let limits = crate::observation::persistence::ReadLimits {
            max_file_bytes: literal.len(),
            admission: crate::observation::Admission {
                max_frames: 8,
                max_bytes: 4_096,
            },
        };
        assert_eq!(
            import_stored_capture_json(literal, limits, ""),
            Err(ImportError::MissingCoordinate)
        );
        let capture = import_stored_capture_json(literal, limits, "stationary-owner").unwrap();
        assert_eq!(capture.source_bytes, literal);
        assert_eq!(
            capture.source_hash,
            SourceHash(Sha256::digest(literal).into())
        );
        assert_eq!(capture.records.len(), 1);
        assert_eq!(capture.records[0].observer, "stationary-owner");
        assert_eq!(capture.records[0].boot_id, Some(1));
        assert_eq!(capture.records[0].source_uptime_ms, Some(2));
        assert_eq!(capture.records[0].rssi_dbm, Some(-70.0));
        assert_eq!(capture.records[0].snr_db, Some(12.5));
        assert_eq!(
            capture.issues,
            vec![
                CaptureIssue::Gap {
                    boot_id: 2,
                    first_missing: 8,
                    count: 4,
                },
                CaptureIssue::CollectorDisconnected,
            ]
        );

        let survey = Survey {
            schema_version: 1,
            tracks: vec![],
            captures: vec![capture],
            settings: AnalysisSettings {
                clock_mappings: vec![],
                interpolation_gap_ms: 1_000,
                freshness_window_ms: 2_000,
                spatial_bucket_m: 25,
                selected_sources: vec![],
            },
        };
        let encoded = export_survey(&survey).unwrap();
        assert_eq!(import_survey(&encoded, encoded.len()).unwrap(), survey);

        let mut tampered = survey;
        tampered.captures[0].unknown["stored_capture_schema_1"]["captured_unix_ms"] =
            serde_json::Value::from(2001);
        let encoded = export_survey(&tampered).unwrap();
        assert_eq!(
            import_survey(&encoded, encoded.len()),
            Err(ImportError::MalformedXml)
        );
    }
}
