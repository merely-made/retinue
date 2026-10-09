//! Bounded import of owner-supplied survey tracks.
//!
//! Imported positions are source evidence. They do not authenticate a board,
//! identify a radio event, or turn a host receipt time into RF time.

mod archive;
mod capture;
mod gpx;
mod model;
#[cfg(test)]
mod tests;

pub use archive::{export_survey, import_survey};
pub use capture::{import_capture_json, import_stored_capture_json, reverse_evidence};
pub use gpx::import_gpx;
pub use model::{
    AnalysisSettings, CAPTURE_SCHEMA_VERSION, CaptureIssue, CaptureLimits, CaptureRecord,
    CaptureSourceFormat, ClockMapping, Direction, ImportError, ImportLimits, ImportedCapture,
    ImportedTrack, ReverseEvidence, SourceHash, Survey, TrackPoint,
};
