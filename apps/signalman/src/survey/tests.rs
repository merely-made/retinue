use sha2::{Digest, Sha256};

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
