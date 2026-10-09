use super::*;

#[test]
fn uf2_volume_writer_copies_the_verified_file_without_an_external_helper() {
    let volume = std::env::temp_dir().join(format!("linkboy-uf2-volume-{}", std::process::id()));
    std::fs::create_dir(&volume).unwrap();
    let package = package(FlashRoute::Uf2MassStorage);
    let plan = uf2_volume_plan(volume.to_string_lossy().into_owned());
    let mut process = MockProcess {
        result: Err(ProcessFailure::MissingHelper {
            program: "must not run".into(),
        }),
        progress: Vec::new(),
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
    .expect("built-in writer should not need an external helper");
    let copied = std::fs::read(volume.join("payload.uf2")).unwrap();
    assert_eq!(copied, package.parts()[0].bytes());
    assert_eq!(
        receipt.result,
        crate::receipt::ReceiptResult::ManualCheckRequired
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, FlashEvent::ManualCheckRequired { .. }))
    );
    std::fs::remove_dir_all(volume).unwrap();
}

#[test]
fn retinue_uf2_install_completes_only_after_application_verification() {
    let volume =
        std::env::temp_dir().join(format!("linkboy-retinue-uf2-volume-{}", std::process::id()));
    std::fs::create_dir(&volume).unwrap();
    let package = package_with_manual_check(FlashRoute::Uf2MassStorage, None);
    let plan = uf2_volume_plan(volume.to_string_lossy().into_owned());
    let mut process = MockProcess {
        result: Err(ProcessFailure::MissingHelper {
            program: "must not run".into(),
        }),
        progress: Vec::new(),
    };
    let mut device = MockDevice {
        bootloader: Err(DeviceFailure::Other("must not enter bootloader".into())),
        application: Ok("COM10".into()),
        verification: Ok(ApplicationVerification {
            board: BoardFamily::T114,
            version: "0.0.1".into(),
            region: Some("US915".into()),
            channel: Some("rnode".into()),
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
    .expect("a Retinue UF2 install should verify the returned application");

    assert_eq!(receipt.result, crate::receipt::ReceiptResult::Complete);
    let verify = events
        .iter()
        .position(|event| matches!(event, FlashEvent::VerifyingApplication))
        .unwrap();
    let complete = events
        .iter()
        .position(|event| matches!(event, FlashEvent::Complete { .. }))
        .unwrap();
    assert!(verify < complete);
    std::fs::remove_dir_all(volume).unwrap();
}

#[test]
fn uf2_disconnect_after_full_write_is_a_bootloader_acknowledgement() {
    let destination = std::env::temp_dir().join("payload.uf2");
    for code in [55, 433] {
        let error = std::io::Error::from_raw_os_error(code);
        assert!(uf2_volume_ejected_after_write(&destination, &error));
    }
}
