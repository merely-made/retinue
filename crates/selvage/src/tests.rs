use super::*;

#[test]
fn known_sync_words_have_the_documented_sx126x_encoding() {
    assert_eq!(sx126x_sync_word(0x34), [0x34, 0x44]);
    assert_eq!(sx126x_sync_word(0x12), [0x14, 0x24]);
    assert_eq!(sx126x_sync_word(MESHTASTIC_SYNC_WORD), [0x24, 0xb4]);
}

#[test]
fn long_fast_keeps_frequency_a_board_setting() {
    let profile = PhyProfile::meshtastic_long_fast(906_875_000);
    assert_eq!(profile.frequency_hz, 906_875_000);
    assert_eq!(profile.sync_word, MESHTASTIC_SYNC_WORD);
    assert_eq!(profile.preamble_symbols, 16);
}

#[test]
fn meshcore_profile_tracks_runtime_radio_settings_and_preamble_rule() {
    let slow = PhyProfile::meshcore(915_000_000, 250_000, 10, 5);
    assert_eq!(slow.sync_word, MESHCORE_SYNC_WORD);
    assert_eq!(slow.preamble_symbols, 16);
    assert!(slow.crc);

    let fast = PhyProfile::meshcore(915_000_000, 62_500, 8, 5);
    assert_eq!(fast.preamble_symbols, 32);
}

#[test]
fn runtime_config_round_trips_all_profile_fields() {
    let mut profile = PhyProfile::meshtastic_long_fast(906_875_000);
    profile.sync_word = 0x12;
    profile.invert_iq = true;
    profile.tx_power_dbm = 11;
    let command = encode_config_command(profile).unwrap();
    assert_eq!(command[0], CMD_CONFIG);
    assert_eq!(decode_config_command(&command), Ok(profile));
}

#[test]
fn runtime_config_rejects_invalid_profiles() {
    let mut profile = PhyProfile::meshtastic_long_fast(906_875_000);
    profile.spreading_factor = 13;
    assert_eq!(
        encode_config_command(profile),
        Err(ProfileError::SpreadingFactor)
    );
}

#[test]
fn command_stream_reassembles_snapshot_at_every_byte_boundary() {
    let payload = [1, 15, 0, 0, 0x55, 0xaa];
    let mut wire = [0_u8; MAX_UI_SNAPSHOT_COMMAND_LEN];
    let wire_len = encode_ui_snapshot_command(&payload, &mut wire).unwrap();

    let mut stream = CommandStream::new();
    let mut command = [0_u8; MAX_COMMAND_LEN];
    let mut event = CommandEvent::Pending;
    for byte in wire[..wire_len].iter().copied() {
        event = stream.push(byte, &mut command);
    }
    assert_eq!(
        event,
        CommandEvent::Complete {
            kind: CommandKind::UiSnapshot,
            len: wire_len - 1,
        }
    );
    assert_eq!(&command[..wire_len - 1], &wire[..wire_len - 1]);
    let mut decoded = [0_u8; MAX_UI_SNAPSHOT_LEN];
    let decoded_len = decode_ui_snapshot_command(&command[..wire_len - 1], &mut decoded).unwrap();
    assert_eq!(&decoded[..decoded_len], &payload);
    assert!(stream.is_boundary());
}

#[test]
fn rejected_oversized_snapshot_does_not_consume_following_config() {
    let declared = MAX_UI_SNAPSHOT_LEN + 1;
    let profile = PhyProfile::meshtastic_long_fast(906_875_000);
    let config = encode_config_command(profile).unwrap();
    let mut stream = CommandStream::new();
    let mut command = [0_u8; MAX_COMMAND_LEN];
    let mut events = [CommandEvent::Pending; 2];
    let mut count = 0;

    for byte in core::iter::once(CMD_UI_SNAPSHOT)
        .chain(core::iter::repeat_n(b'a', declared * 2))
        .chain(core::iter::once(WAKE_BYTE))
        .chain(config)
    {
        let event = stream.push(byte, &mut command);
        if event != CommandEvent::Pending {
            events[count] = event;
            count += 1;
        }
    }

    assert_eq!(
        &events[..count],
        &[
            CommandEvent::TooLong {
                kind: CommandKind::UiSnapshot,
                declared,
            },
            CommandEvent::Complete {
                kind: CommandKind::Configure,
                len: CONFIG_COMMAND_LEN,
            },
        ]
    );
    assert_eq!(&command[..CONFIG_COMMAND_LEN], &config);
}

#[test]
fn truncated_snapshot_ends_at_next_wake_without_consuming_config() {
    let mut wire = [0_u8; MAX_UI_SNAPSHOT_COMMAND_LEN];
    let wire_len = encode_ui_snapshot_command(&[1, 2, 3], &mut wire).unwrap();
    let profile = PhyProfile::meshtastic_long_fast(906_875_000);
    let config = encode_config_command(profile).unwrap();
    let mut stream = CommandStream::new();
    let mut command = [0_u8; MAX_COMMAND_LEN];
    let mut saw_truncated = false;
    let mut saw_config = false;

    for byte in wire[..wire_len - 2]
        .iter()
        .copied()
        .chain(core::iter::once(WAKE_BYTE))
        .chain(config)
    {
        match stream.push(byte, &mut command) {
            CommandEvent::Complete {
                kind: CommandKind::UiSnapshot,
                len,
            } => {
                let mut decoded = [0_u8; MAX_UI_SNAPSHOT_LEN];
                assert_eq!(
                    decode_ui_snapshot_command(&command[..len], &mut decoded),
                    Err(UiSnapshotWireError::OddLength)
                );
                saw_truncated = true;
            }
            CommandEvent::Complete {
                kind: CommandKind::Configure,
                len,
            } => {
                assert_eq!(len, CONFIG_COMMAND_LEN);
                assert_eq!(&command[..len], &config);
                saw_config = true;
            }
            CommandEvent::Pending => {}
            other => panic!("unexpected command event {other:?}"),
        }
    }

    assert!(saw_truncated);
    assert!(saw_config);
}

#[test]
fn wake_prefix_is_ignored_only_at_a_command_boundary() {
    let mut stream = CommandStream::new();
    let mut command = [0_u8; MAX_COMMAND_LEN];
    for _ in 0..8 {
        assert_eq!(stream.push(WAKE_BYTE, &mut command), CommandEvent::Pending);
    }
    let bytes = [CMD_TX, 1, 0, WAKE_BYTE];
    let mut event = CommandEvent::Pending;
    for byte in bytes {
        event = stream.push(byte, &mut command);
    }
    assert_eq!(
        event,
        CommandEvent::Complete {
            kind: CommandKind::Transmit,
            len: bytes.len(),
        }
    );
    assert_eq!(&command[..bytes.len()], &bytes);
}

#[test]
fn observation_command_is_delimited_and_recovers_after_truncation() {
    use observation::{MAX_OBSERVATION_COMMAND_LEN, Request, decode_request, encode_request};
    let request = Request::Cursor {
        request_id: 7,
        boot_id: 9,
        after_sequence: 12,
    };
    let mut wire = [0; MAX_OBSERVATION_COMMAND_LEN];
    let len = encode_request(request, &mut wire);
    let mut stream = CommandStream::new();
    let mut command = [0; MAX_COMMAND_LEN];
    for byte in &wire[..len - 1] {
        assert_eq!(stream.push(*byte, &mut command), CommandEvent::Pending);
    }
    assert_eq!(
        stream.push(0, &mut command),
        CommandEvent::Complete {
            kind: CommandKind::Observation,
            len: len - 1,
        }
    );
    assert_eq!(decode_request(&command[..len - 1]), Ok(request));
    for byte in &wire[..9] {
        assert_eq!(stream.push(*byte, &mut command), CommandEvent::Pending);
    }
    assert_eq!(
        stream.push(0, &mut command),
        CommandEvent::Complete {
            kind: CommandKind::Observation,
            len: 9,
        }
    );
    assert!(decode_request(&command[..9]).is_err());
    for byte in &wire[..len - 1] {
        assert_eq!(stream.push(*byte, &mut command), CommandEvent::Pending);
    }
    assert_eq!(
        stream.push(0, &mut command),
        CommandEvent::Complete {
            kind: CommandKind::Observation,
            len: len - 1,
        }
    );
    assert_eq!(decode_request(&command[..len - 1]), Ok(request));
}
