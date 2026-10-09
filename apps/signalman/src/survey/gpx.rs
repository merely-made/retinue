//! Bounded GPX track import.

use quick_xml::Reader;
use quick_xml::events::Event;
use sha2::{Digest, Sha256};

use super::{ImportError, ImportLimits, ImportedTrack, SourceHash, TrackPoint};

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

pub(super) fn valid_utc_time(value: &str) -> bool {
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
