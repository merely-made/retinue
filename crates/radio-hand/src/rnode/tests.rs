use super::*;

#[test]
fn the_captured_init_commands_decode() {
    assert_eq!(
        decode(&[cmd::DETECT, DETECT_REQ]),
        Some(Command::Detect(0x73))
    );
    assert_eq!(
        decode(&[cmd::FW_VERSION, 0x00]),
        Some(Command::FirmwareVersion)
    );
    assert_eq!(decode(&[cmd::PLATFORM, 0x00]), Some(Command::Platform));
    assert_eq!(decode(&[cmd::MCU, 0x00]), Some(Command::Mcu));
    // 0x3689cac0 = 915 MHz, 0x0001e848 = 125 kHz, both big-endian on the wire.
    assert_eq!(
        decode(&[cmd::FREQUENCY, 0x36, 0x89, 0xca, 0xc0]),
        Some(Command::Frequency(915_000_000))
    );
    assert_eq!(
        decode(&[cmd::BANDWIDTH, 0x00, 0x01, 0xe8, 0x48]),
        Some(Command::Bandwidth(125_000))
    );
    assert_eq!(decode(&[cmd::TXPOWER, 0x07]), Some(Command::TxPower(7)));
    assert_eq!(decode(&[cmd::SF, 0x08]), Some(Command::SpreadingFactor(8)));
    assert_eq!(decode(&[cmd::CR, 0x05]), Some(Command::CodingRate(5)));
    assert_eq!(
        decode(&[cmd::RADIO_STATE, 0x01]),
        Some(Command::RadioState(true))
    );
}

#[test]
fn airtime_locks_decode_and_leave_is_known() {
    let lock = decode(&[cmd::ST_ALOCK, 0x0D, 0x16]).unwrap();
    assert_eq!(
        lock,
        Command::AirtimeLock {
            long: false,
            centi: 3_350
        }
    );
    let (marker, payload) = answer(&lock).unwrap();
    assert_eq!(
        (marker, payload.as_slice()),
        (cmd::ST_ALOCK, &[0, 0][..]),
        "echoed as no limit, since none is enforced"
    );
    assert_eq!(decode(&[cmd::LEAVE, 0xFF]), Some(Command::Leave));
    assert_eq!(decode(&[cmd::READY, 0x01]), Some(Command::Ready));
    assert_eq!(answer(&Command::Leave), None);
    assert_eq!(
        decode(&[cmd::LT_ALOCK, 0x01]),
        Some(Command::Unhandled(cmd::LT_ALOCK))
    );
}

#[test]
fn a_truncated_setting_is_unhandled_rather_than_guessed() {
    assert_eq!(
        decode(&[cmd::FREQUENCY, 0x36, 0x89]),
        Some(Command::Unhandled(cmd::FREQUENCY))
    );
    assert_eq!(decode(&[]), None);
}

/// An empty DATA frame is a command with an empty payload, not a missing one: the caller
/// decides what to do with it, and refusing it here would hide it.
#[test]
fn data_carries_its_payload_verbatim() {
    assert_eq!(
        decode(&[cmd::DATA, 1, 2, 3]),
        Some(Command::Data(&[1, 2, 3]))
    );
    assert_eq!(decode(&[cmd::DATA]), Some(Command::Data(&[])));
}

#[test]
fn a_profile_is_withheld_until_every_field_the_host_controls_has_arrived() {
    let mut pending = Pending::new();
    for command in [
        Command::Frequency(915_000_000),
        Command::Bandwidth(125_000),
        Command::TxPower(7),
        Command::SpreadingFactor(8),
    ] {
        assert!(pending.accept(&command));
        assert!(pending.profile().is_none(), "still incomplete");
    }
    assert!(pending.accept(&Command::CodingRate(5)));

    let profile = pending.profile().expect("complete");
    assert_eq!(profile.frequency_hz, 915_000_000);
    assert_eq!(profile.bandwidth_hz, 125_000);
    assert_eq!(profile.spreading_factor, 8);
    assert_eq!(profile.coding_rate_denominator, 5);
    assert_eq!(profile.tx_power_dbm, 7);
    assert_eq!(profile.sync_word, SYNC_WORD);
}

#[test]
fn non_settings_commands_are_not_accepted_as_settings() {
    let mut pending = Pending::new();
    assert!(!pending.accept(&Command::Detect(DETECT_REQ)));
    assert!(!pending.accept(&Command::Data(&[1])));
    assert!(pending.profile().is_none());
}

/// The stat triplet's encodings, against the values the RX capture carried: raw 0x61 is
/// -60 dBm and raw 0x3b is 14.75 dB.
#[test]
fn signal_reports_encode_as_the_capture_did() {
    assert_eq!(rssi_wire(-60), 0x61);
    assert_eq!(snr_wire(14), 56);
    assert_eq!(rssi_wire(-200), 0, "clamped rather than wrapped");
    assert_eq!(snr_wire(100), 127);
}

#[test]
fn frames_encode_with_their_command_byte() {
    let mut out = [0_u8; 16];
    let len = encode(cmd::DETECT, &[DETECT_RESP], &mut out).unwrap();
    assert_eq!(
        &out[..len],
        &[kiss::FEND, cmd::DETECT, DETECT_RESP, kiss::FEND]
    );
}

#[test]
fn the_worst_case_encoding_bound_holds() {
    let payload = [kiss::FEND; 8];
    let mut out = [0_u8; encoded_max(8)];
    let len = encode(kiss::FESC, &payload, &mut out).unwrap();
    assert_eq!(len, out.len(), "every byte escaped is the worst case");
}

/// Only a radio failure reaches the host as an `ERROR` RNS acts on; per-frame refusals
/// would otherwise tear its interface down.
#[test]
fn only_radio_failures_are_reported_as_errors() {
    assert_eq!(tx_error(selvage::TX_RADIO_FAULT), Some(error::TX_FAILED));
    assert_eq!(tx_error(selvage::TX_TIMEOUT), Some(error::MODEM_TIMEOUT));
    for code in [
        selvage::TX_ACCEPTED,
        selvage::TX_TOO_LONG,
        selvage::TX_NO_REGION,
        selvage::TX_OVER_DUTY,
        selvage::TX_CHANNEL_BUSY,
    ] {
        assert_eq!(tx_error(code), None);
    }
}
