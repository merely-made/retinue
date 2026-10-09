//! Whole-survey export and revalidating reimport.

use sha2::{Digest, Sha256};

use super::gpx::valid_utc_time;
use super::{
    CAPTURE_SCHEMA_VERSION, CaptureIssue, CaptureLimits, CaptureSourceFormat, ImportError,
    ImportLimits, ImportedTrack, SourceHash, Survey, import_capture_json, import_gpx,
    import_stored_capture_json,
};

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
