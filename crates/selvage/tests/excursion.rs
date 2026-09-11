use selvage::{
    CMD_CONFIG, CMD_EXCURSION, CMD_OBSERVATION, CMD_TX, CMD_UI_SNAPSHOT, CommandEvent, CommandKind,
    CommandStream, EVENT_CONFIG, EVENT_EXCURSION, EVENT_RX, EVENT_TX, EXCURSION_COMMAND_LEN,
    MAX_COMMAND_LEN, MESHCORE_SYNC_WORD, PhyProfile, decode_config_command,
    decode_excursion_command, encode_config_command,
};

#[test]
fn literal_wire_fixture_has_fixed_profile_and_little_endian_duration() {
    let bytes = [
        0x06, 0x78, 0xd0, 0x0d, 0x36, 0x90, 0xd0, 0x03, 0x00, 0x0b, 0x05, 0x10, 0x00, 0x12, 0x03,
        0x07, 0xd0, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let (profile, duration) = decode_excursion_command(&bytes).unwrap();
    assert_eq!(profile.frequency_hz, 906_875_000);
    assert_eq!(profile.bandwidth_hz, 250_000);
    assert_eq!(profile.spreading_factor, 11);
    assert_eq!(profile.coding_rate_denominator, 5);
    assert_eq!(profile.preamble_symbols, 16);
    assert_eq!(profile.sync_word, 0x12);
    assert_eq!(profile.tx_power_dbm, 7);
    assert!(profile.explicit_header && profile.crc && !profile.invert_iq);
    assert_eq!(duration, 2_000);
}

fn fixture() -> (PhyProfile, [u8; EXCURSION_COMMAND_LEN]) {
    let mut profile = PhyProfile::meshcore(906_875_000, 250_000, 7, 5);
    profile.sync_word = CMD_EXCURSION;
    let mut command = [0; EXCURSION_COMMAND_LEN];
    let config = encode_config_command(profile).unwrap();
    command[..config.len()].copy_from_slice(&config);
    command[0] = CMD_EXCURSION;
    command[EXCURSION_COMMAND_LEN - 8..].copy_from_slice(&[
        CMD_TX,
        CMD_CONFIG,
        CMD_UI_SNAPSHOT,
        CMD_EXCURSION,
        EVENT_RX,
        EVENT_TX,
        EVENT_CONFIG,
        EVENT_EXCURSION,
    ]);
    (profile, command)
}

#[test]
fn literal_fixture_decodes_profile_and_little_endian_duration() {
    let (profile, mut command) = fixture();
    command[EXCURSION_COMMAND_LEN - 8..].copy_from_slice(&123_456_u64.to_le_bytes());
    assert_eq!(decode_excursion_command(&command), Ok((profile, 123_456)));
    assert_eq!(
        decode_config_command(&encode_config_command(profile).unwrap()),
        Ok(profile)
    );
}

#[test]
fn excursion_stream_reassembles_at_every_split_position() {
    let (_, command) = fixture();
    for split in 0..=command.len() {
        let mut stream = CommandStream::new();
        let mut output = [0; MAX_COMMAND_LEN];
        let mut completions = 0;
        for byte in command.iter().take(split) {
            let event = stream.push(*byte, &mut output);
            if event
                == (CommandEvent::Complete {
                    kind: CommandKind::Excursion,
                    len: EXCURSION_COMMAND_LEN,
                })
            {
                completions += 1;
            } else {
                assert_eq!(event, CommandEvent::Pending, "split at {split}");
            }
        }
        for byte in command.iter().skip(split) {
            let event = stream.push(*byte, &mut output);
            if event
                == (CommandEvent::Complete {
                    kind: CommandKind::Excursion,
                    len: EXCURSION_COMMAND_LEN,
                })
            {
                completions += 1;
            } else {
                assert_eq!(event, CommandEvent::Pending, "split at {split}");
            }
        }
        assert_eq!(completions, 1, "split at {split}");
        assert_eq!(&output[..command.len()], &command);
        assert!(stream.is_boundary());
    }
}

#[test]
fn embedded_markers_are_payload_and_do_not_start_commands() {
    let (_, command) = fixture();
    let mut stream = CommandStream::new();
    let mut output = [0; MAX_COMMAND_LEN];
    let mut complete = 0;
    for byte in command {
        match stream.push(byte, &mut output) {
            CommandEvent::Pending => {}
            CommandEvent::Complete { kind, len } => {
                complete += 1;
                assert_eq!(kind, CommandKind::Excursion);
                assert_eq!(len, EXCURSION_COMMAND_LEN);
            }
            other => panic!("embedded marker produced {other:?}"),
        }
    }
    assert_eq!(complete, 1);
    assert_eq!(
        decode_excursion_command(&output[..EXCURSION_COMMAND_LEN])
            .unwrap()
            .1,
        u64::from_le_bytes([
            CMD_TX,
            CMD_CONFIG,
            CMD_UI_SNAPSHOT,
            CMD_EXCURSION,
            EVENT_RX,
            EVENT_TX,
            EVENT_CONFIG,
            EVENT_EXCURSION,
        ])
    );
}

#[test]
fn ordinary_observation_marker_still_uses_observation_framing() {
    let mut stream = CommandStream::new();
    let mut output = [0; MAX_COMMAND_LEN];
    assert_eq!(
        stream.push(CMD_OBSERVATION, &mut output),
        CommandEvent::Pending
    );
    assert_eq!(stream.push(0x42, &mut output), CommandEvent::Pending);
    assert_eq!(
        stream.push(0, &mut output),
        CommandEvent::Complete {
            kind: CommandKind::Observation,
            len: 2,
        }
    );
}

#[test]
fn excursion_decoder_rejects_wrong_length_and_invalid_profile() {
    let (_, command) = fixture();
    assert!(decode_excursion_command(&command[..command.len() - 1]).is_err());
    let mut too_long = [0; EXCURSION_COMMAND_LEN + 1];
    too_long[..command.len()].copy_from_slice(&command);
    assert!(decode_excursion_command(&too_long).is_err());
    let mut invalid = command;
    invalid[1..5].fill(0);
    assert!(decode_excursion_command(&invalid).is_err());
}

#[test]
fn command_and_event_markers_are_unique() {
    let markers = [
        CMD_TX,
        CMD_CONFIG,
        CMD_UI_SNAPSHOT,
        CMD_OBSERVATION,
        CMD_EXCURSION,
        EVENT_RX,
        EVENT_TX,
        EVENT_CONFIG,
        EVENT_EXCURSION,
    ];
    for (index, marker) in markers.iter().enumerate() {
        assert!(
            !markers[..index].contains(marker),
            "duplicate marker {marker:#x}"
        );
    }
    assert_eq!(MESHCORE_SYNC_WORD, 0x12);
}
