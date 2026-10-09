use super::*;

#[test]
fn rediscovery_rejects_an_unrelated_retinue_family() {
    let expected = ExpectedApplication {
        board: BoardFamily::T114,
        version: "0.0.1".into(),
        manual_check: None,
    };
    let v4 = crate::Found {
        port: "COM6".into(),
        board: Some(crate::Board::HeltecV4),
        banner: "tulle/heltec-v4 phy online".into(),
        region: None,
        channel: None,
    };
    let t114 = crate::Found {
        port: "COM10".into(),
        board: Some(crate::Board::T114),
        banner: "tulle/t114 phy online".into(),
        region: None,
        channel: None,
    };
    assert!(!matches_expected_family(&v4, &expected));
    assert!(matches_expected_family(&t114, &expected));
}

#[test]
fn rediscovery_accepts_application_on_former_bootloader_port() {
    let expected = ExpectedApplication {
        board: BoardFamily::T114,
        version: "0.0.1".into(),
        manual_check: None,
    };
    let ports = vec!["COM10".to_string()];
    let application = select_application_port(&ports, "COM3", "COM10", &expected, |port, _| {
        port == "COM10"
    })
    .expect("one expected application port is unambiguous");

    assert_eq!(application.as_deref(), Some("COM10"));
}
