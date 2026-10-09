use super::*;

#[test]
fn success_emits_complete_only_after_application_verification() {
    let plan = plan(FlashRoute::EspRom);
    let package = package(FlashRoute::EspRom);
    let mut process = MockProcess {
        result: Ok(ProcessOutput {
            diagnostics: "write 100%".into(),
        }),
        progress: vec![ProcessProgress {
            written: 100,
            total: 100,
        }],
    };
    let mut device = success_device();
    let mut events = Vec::new();
    let receipt = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .unwrap();
    assert_eq!(receipt.result, crate::receipt::ReceiptResult::Complete);
    let complete = events
        .iter()
        .position(|event| matches!(event, FlashEvent::Complete { .. }))
        .unwrap();
    let verify = events
        .iter()
        .position(|event| matches!(event, FlashEvent::VerifyingApplication))
        .unwrap();
    assert!(verify < complete);
}

#[test]
fn an_explicit_t114_dfu_port_skips_bootloader_entry() {
    let plan = plan_with_transport(
        FlashRoute::AdafruitDfu,
        DeviceTransport::SerialDfuPort("COM10".into()),
    );
    let package = package(FlashRoute::AdafruitDfu);
    let mut process = RecordingProcess::default();
    let mut device = MockDevice {
        bootloader: Err(DeviceFailure::Other(
            "an already-DFU route must not enter the bootloader".into(),
        )),
        application: Ok("COM10".into()),
        verification: Ok(ApplicationVerification {
            board: BoardFamily::T114,
            version: "0.0.1".into(),
            region: Some("US915".into()),
            channel: Some("modem".into()),
        }),
    };
    let mut events = Vec::new();

    let receipt = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect("the selected DFU port should execute directly");

    assert_eq!(receipt.result, crate::ReceiptResult::Complete);
    assert_eq!(process.calls.len(), 1);
    assert_eq!(process.calls[0][5], "COM10");
    assert!(!events.iter().any(|event| matches!(
        event,
        FlashEvent::EnteringBootloader | FlashEvent::Rediscovering
    )));
}

#[test]
fn missing_helper_is_preserved_as_a_structured_process_failure() {
    let plan = plan(FlashRoute::EspRom);
    let package = package(FlashRoute::EspRom);
    let mut process = MockProcess {
        result: Err(ProcessFailure::MissingHelper {
            program: "espflash".into(),
        }),
        progress: vec![],
    };
    let mut device = success_device();
    let mut events = Vec::new();
    let error = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect_err("missing helper must stop before transfer");
    assert!(matches!(
        error,
        ExecutionError::Process(ProcessFailure::MissingHelper { .. })
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, FlashEvent::Complete { .. }))
    );
}

#[test]
fn helper_timeout_is_preserved_as_a_structured_process_failure() {
    let plan = plan(FlashRoute::EspRom);
    let package = package(FlashRoute::EspRom);
    let mut process = MockProcess {
        result: Err(ProcessFailure::Timeout {
            program: "espflash".into(),
        }),
        progress: vec![],
    };
    let mut device = success_device();
    let mut events = Vec::new();
    let error = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect_err("a helper timeout before progress must stop the transfer");
    assert!(matches!(
        error,
        ExecutionError::Process(ProcessFailure::Timeout { .. })
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, FlashEvent::Complete { .. }))
    );
}

#[test]
fn helper_failure_after_progress_requires_recovery() {
    let plan = plan(FlashRoute::EspRom);
    let package = package(FlashRoute::EspRom);
    let mut process = MockProcess {
        result: Err(ProcessFailure::Failed {
            program: "espflash".into(),
            diagnostics: "write failed".into(),
        }),
        progress: vec![ProcessProgress {
            written: 50,
            total: 100,
        }],
    };
    let mut device = success_device();
    let mut events = Vec::new();
    let error = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect_err("partial transfer must require recovery");
    assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
    assert!(events.iter().any(|event| matches!(
        event,
        FlashEvent::RecoveryRequired {
            facts: RecoveryFacts {
                write_started: true,
                ..
            },
            ..
        }
    )));
}

#[test]
fn disappearing_device_and_post_write_silence_are_recovery_events() {
    let plan = plan(FlashRoute::EspRom);
    let package = package(FlashRoute::EspRom);
    let mut process = MockProcess {
        result: Ok(ProcessOutput {
            diagnostics: String::new(),
        }),
        progress: vec![ProcessProgress {
            written: 100,
            total: 100,
        }],
    };
    let mut device = MockDevice {
        bootloader: Ok("DFU1".into()),
        application: Err(DeviceFailure::Disappeared("COM7".into())),
        verification: Err(DeviceFailure::Silence("COM7".into())),
    };
    let mut events = Vec::new();
    let error = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect_err("lost application must require recovery");
    assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
    assert!(events.iter().any(|event| matches!(
        event,
        FlashEvent::RecoveryRequired {
            facts: RecoveryFacts {
                stage: ExecutionStage::Rebooting,
                ..
            },
            ..
        }
    )));
}

#[test]
fn unexpected_application_port_is_a_recovery_event() {
    let plan = plan(FlashRoute::EspRom);
    let package = package(FlashRoute::EspRom);
    let mut process = MockProcess {
        result: Ok(ProcessOutput {
            diagnostics: "write 100%".into(),
        }),
        progress: vec![ProcessProgress {
            written: 100,
            total: 100,
        }],
    };
    let mut device = MockDevice {
        bootloader: Ok("DFU1".into()),
        application: Err(DeviceFailure::UnexpectedPort {
            expected: "COM7".into(),
            found: "COM9".into(),
        }),
        verification: Err(DeviceFailure::Silence("COM9".into())),
    };
    let mut events = Vec::new();
    let error = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect_err("a contradictory application port must require recovery");
    assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
    assert!(events.iter().any(|event| matches!(
        event,
        FlashEvent::RecoveryRequired {
            facts: RecoveryFacts {
                stage: ExecutionStage::Rebooting,
                last_known_port: Some(port),
                ..
            },
            ..
        } if port == "COM7"
    )));
}

#[test]
fn wrong_application_after_successful_transfer_requires_recovery() {
    let plan = plan(FlashRoute::EspRom);
    let package = package(FlashRoute::EspRom);
    let mut process = MockProcess {
        result: Ok(ProcessOutput {
            diagnostics: String::new(),
        }),
        progress: vec![ProcessProgress {
            written: 100,
            total: 100,
        }],
    };
    let mut device = MockDevice {
        bootloader: Ok("DFU1".into()),
        application: Ok("COM7".into()),
        verification: Ok(ApplicationVerification {
            board: BoardFamily::HeltecV4,
            version: "0.0.2".into(),
            region: None,
            channel: None,
        }),
    };
    let mut events = Vec::new();
    let error = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect_err("wrong application must not be complete");
    assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, FlashEvent::Complete { .. }))
    );
}

#[test]
fn route_commands_select_the_new_t114_port() {
    let command = adafruit_dfu::command("COM9", std::path::Path::new("firmware.zip"));
    assert_eq!(command[5], "COM9");
    let command = esp_rom::command("COM7", std::path::Path::new("firmware.elf"));
    assert_eq!(command[2], "COM7");
}

#[test]
fn sparse_esp_package_writes_every_part_then_requires_its_own_manual_check() {
    let plan = plan(FlashRoute::EspRom);
    let package = sparse_esp_package();
    let mut process = RecordingProcess::default();
    let mut device = success_device();
    let mut events = Vec::new();
    let receipt = execute_plan(
        &plan,
        &package,
        &mut process,
        &mut device,
        Duration::from_secs(1),
        &mut |event| events.push(event),
    )
    .expect("a validated sparse package should execute every part");
    assert_eq!(
        receipt.result,
        crate::receipt::ReceiptResult::ManualCheckRequired
    );
    assert_eq!(process.calls.len(), 3);
    assert!(
        process.calls[0]
            .windows(2)
            .any(|pair| pair == ["--before", "usb-reset"])
    );
    assert!(
        process.calls[1]
            .windows(2)
            .any(|pair| pair == ["--before", "no-reset"])
    );
    assert!(
        process.calls[2]
            .windows(2)
            .any(|pair| pair == ["--after", "watchdog-reset"])
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, FlashEvent::ManualCheckRequired { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, FlashEvent::VerifyingApplication))
    );
}
