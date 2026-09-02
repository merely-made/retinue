use heapless::Vec;
use radio_hand::control::*;

fn bytes<const N: usize>(value: &[u8]) -> Vec<u8, N> {
    Vec::try_from(value).unwrap()
}

fn request() -> Request {
    Request {
        transaction: TransactionId([0x30; ID_LEN]),
        transaction_sequence: 9,
        expected_generation: ConfigGeneration(7),
        operation: Operation::StageConfiguration,
        arguments: bytes(&[1, 2, 3]),
    }
}

#[test]
fn canonical_request_and_provisional_response_round_trip() {
    let request = request();
    let mut request_out = [0; MAX_REQUEST_LEN];
    let request_len = encode_request(&request, &mut request_out).unwrap();
    assert_eq!(&request_out[..request_len], GOLDEN_REQUEST);
    assert_eq!(decode_request(&GOLDEN_REQUEST).unwrap(), request);

    let response = Response {
        node: NodeId([0x10; ID_LEN]),
        transaction: TransactionId([0x30; ID_LEN]),
        known_good_generation: ConfigGeneration(7),
        effective_generation: Some(ConfigGeneration(8)),
        body: ResponseBody::Provisional {
            deadline_ms: 9,
            commit_token: [0x40; COMMIT_TOKEN_LEN],
            result: bytes(&[0xC0, 0xDE]),
        },
    };
    let mut response_out = [0; MAX_RESPONSE_LEN];
    let response_len = encode_response(&response, &mut response_out).unwrap();
    assert_eq!(&response_out[..response_len], GOLDEN_RESPONSE);
    assert_eq!(decode_response(&GOLDEN_RESPONSE).unwrap(), response);
}

#[test]
fn frame_ceilings_and_bounded_capabilities_hold() {
    assert_eq!(MAX_REQUEST_LEN, retinue::command::MAX_PAYLOAD);
    assert_eq!(MAX_RESPONSE_LEN, retinue::command::MAX_PAYLOAD);
    let mut request = request();
    request.arguments = bytes(&[0x55; MAX_ARGUMENTS]);
    let mut out = [0; MAX_REQUEST_LEN];
    assert_eq!(encode_request(&request, &mut out), Ok(MAX_REQUEST_LEN));

    let response = Response {
        node: NodeId([0x10; ID_LEN]),
        transaction: TransactionId([0x30; ID_LEN]),
        known_good_generation: ConfigGeneration(7),
        effective_generation: Some(ConfigGeneration(8)),
        body: ResponseBody::Provisional {
            deadline_ms: 9,
            commit_token: [0x40; COMMIT_TOKEN_LEN],
            result: bytes(&[0x50; MAX_RESULT]),
        },
    };
    let mut exact = [0; MAX_RESPONSE_LEN];
    assert_eq!(encode_response(&response, &mut exact), Ok(MAX_RESPONSE_LEN));
    let mut short = [0; MAX_RESPONSE_LEN - 1];
    assert_eq!(
        encode_response(&response, &mut short),
        Err(EncodeError::BufferTooSmall)
    );

    let mut capabilities = Capabilities::empty(BoardClass::Hybrid);
    capabilities.controller_role = Some(ControllerRole::Owner);
    for slot in 0..MAX_IMAGE_SLOTS as u8 {
        capabilities
            .image_slots
            .push(ImageSlot {
                slot,
                kind: ImageKind::Retinue,
                verified: true,
                active: slot == 0,
                trial: false,
            })
            .unwrap();
    }
    for _ in 0..MAX_ADAPTERS {
        capabilities
            .adapters
            .push(AdapterCapability {
                adapter: ResidentAdapter::Reticulum,
                enabled: true,
                radio_leases: 1,
            })
            .unwrap();
    }
    for _ in 0..MAX_RADIOS {
        capabilities
            .radios
            .push(RadioCapability {
                radio: RadioKind::Sx1262,
                simultaneous_receive_profiles: 1,
                tx: true,
            })
            .unwrap();
    }
    for _ in 0..MAX_CARRIERS {
        capabilities
            .carriers
            .push(CarrierCapability {
                carrier: ManagementCarrier::Usb,
                authenticated: true,
                max_frame: 256,
            })
            .unwrap();
    }
    for _ in 0..MAX_RECOVERY_PATHS {
        capabilities
            .recovery_paths
            .push(RecoveryPath {
                carrier: ManagementCarrier::Usb,
                enabled: true,
                remote: true,
                physical_presence: true,
            })
            .unwrap();
    }
    let response = Response {
        node: NodeId([0x10; ID_LEN]),
        transaction: TransactionId([0x30; ID_LEN]),
        known_good_generation: ConfigGeneration(7),
        effective_generation: None,
        body: ResponseBody::Capabilities(capabilities),
    };
    let len = encode_response(&response, &mut exact).unwrap();
    assert!(len <= MAX_RESPONSE_LEN);
    assert_eq!(decode_response(&exact[..len]).unwrap(), response);
}

#[test]
fn codec_rejects_bad_version_tags_lengths_and_tails() {
    let mut out = [0; MAX_REQUEST_LEN];
    let len = encode_request(&request(), &mut out).unwrap();
    let mut frame = out[..len].to_vec();
    frame[4] = 3;
    assert_eq!(
        decode_request(&frame),
        Err(DecodeError::UnsupportedVersion(3))
    );
    frame[4] = VERSION;
    frame[37] = 0xFF;
    assert_eq!(
        decode_request(&frame),
        Err(DecodeError::UnknownOperation(0xFF))
    );
    frame[37] = Operation::StageConfiguration as u8;
    frame[38] = MAX_ARGUMENTS as u8 + 1;
    assert_eq!(
        decode_request(&frame),
        Err(DecodeError::OversizedField {
            declared: MAX_ARGUMENTS + 1,
            maximum: MAX_ARGUMENTS
        })
    );
    assert_eq!(decode_request(&out[..len - 1]), Err(DecodeError::Truncated));
    frame[38] = 3;
    frame.push(0);
    assert_eq!(decode_request(&frame), Err(DecodeError::TrailingBytes));
}

#[test]
fn observation_and_debug_redaction_are_explicit() {
    let body = ResponseBody::Observed(bytes(b"current"));
    assert_eq!(body.disposition(), Disposition::Observed);
    let observed = Response {
        node: NodeId([0x10; ID_LEN]),
        transaction: TransactionId([0x30; ID_LEN]),
        known_good_generation: ConfigGeneration(7),
        effective_generation: None,
        body,
    };
    let mut observed_out = [0; MAX_RESPONSE_LEN];
    let observed_len = encode_response(&observed, &mut observed_out).unwrap();
    assert_eq!(
        decode_response(&observed_out[..observed_len]).unwrap(),
        observed
    );
    let request = Request {
        arguments: bytes(b"wifi-secret-marker"),
        ..request()
    };
    let provisional = ResponseBody::Provisional {
        deadline_ms: 9,
        commit_token: [0xA5; COMMIT_TOKEN_LEN],
        result: bytes(b"result-secret-marker"),
    };
    let request_debug = format!("{request:?}");
    let response_debug = format!("{provisional:?}");
    assert!(!request_debug.contains("wifi-secret-marker"));
    assert!(!response_debug.contains("result-secret-marker"));
    assert!(!response_debug.contains("165"));
    assert!(response_debug.contains("redacted"));
}

#[test]
fn response_rejects_unknown_capability_role() {
    let mut capabilities = Capabilities::empty(BoardClass::Simple);
    capabilities.controller_role = Some(ControllerRole::Owner);
    let response = Response {
        node: NodeId([0x10; ID_LEN]),
        transaction: TransactionId([0x30; ID_LEN]),
        known_good_generation: ConfigGeneration(7),
        effective_generation: None,
        body: ResponseBody::Capabilities(capabilities),
    };
    let mut frame = [0; MAX_RESPONSE_LEN];
    let len = encode_response(&response, &mut frame).unwrap();
    frame[50] = 0xFF;
    assert_eq!(
        decode_response(&frame[..len]),
        Err(DecodeError::UnknownControllerRole(0xFF))
    );
}

#[test]
fn generation_never_wraps_and_read_only_operations_are_identified() {
    assert_eq!(
        ConfigGeneration(7).checked_successor(),
        Ok(ConfigGeneration(8))
    );
    assert_eq!(
        ConfigGeneration(u64::MAX).checked_successor(),
        Err(Refusal::GenerationExhausted)
    );
    for operation in [
        Operation::Capabilities,
        Operation::Status,
        Operation::WifiScan,
        Operation::RecoveryStatus,
    ] {
        assert!(!operation.requires_generation());
    }
    assert!(Operation::Commit.requires_generation());
}
