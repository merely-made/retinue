//! Survey evidence types and import limits.

use serde::{Deserialize, Serialize};

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
