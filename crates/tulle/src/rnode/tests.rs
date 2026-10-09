use super::*;
use crate::lora::CodingRate;

fn params() -> LoRaParams {
    LoRaParams {
        spreading_factor: 7,
        bandwidth_hz: 500_000,
        coding_rate: CodingRate::Cr45,
        frequency_hz: 867_200_000,
        tx_power_dbm: 14,
        preamble_syms: 8,
        explicit_header: true,
        crc: true,
    }
}

fn frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    kiss::Deframer::new(1024).push(bytes, &mut out);
    out
}

/// The device's answers to an init, with `tweak` applied to the echoes.
fn device_echoes(config: &RNodeConfig, tweak: impl Fn(&mut Vec<Vec<u8>>)) -> Vec<u8> {
    let p = config.params;
    let mut echoes = vec![
        vec![cmd::DETECT, DETECT_RESP],
        vec![cmd::FW_VERSION, 1, 86],
        vec![cmd::PLATFORM, 0x80],
        [&[cmd::FREQUENCY][..], &p.frequency_hz.to_be_bytes()].concat(),
        [&[cmd::BANDWIDTH][..], &p.bandwidth_hz.to_be_bytes()].concat(),
        vec![cmd::TXPOWER, p.tx_power_dbm],
        vec![cmd::SF, p.spreading_factor],
        vec![cmd::CR, coding_rate_wire(&p)],
        vec![cmd::RADIO_STATE, 1],
    ];
    tweak(&mut echoes);
    echoes
        .iter()
        .flat_map(|frame| kiss::encode(frame))
        .collect()
}

fn started(config: RNodeConfig) -> RNode {
    let mut rnode = RNode::with_config(config);
    rnode.start();
    rnode.take_outbound();
    rnode
}

fn online(config: RNodeConfig) -> RNode {
    let mut rnode = started(config);
    rnode.on_serial(&device_echoes(&config, |_| {}));
    assert!(rnode.is_online());
    rnode
}

#[test]
fn airtime_locks_are_sent_before_the_radio_comes_up() {
    let config = RNodeConfig {
        st_alock: Some(3_350),
        lt_alock: Some(1_000),
        ..RNodeConfig::new(params())
    };
    let mut rnode = RNode::with_config(config);
    rnode.start();
    let sent = frames(&rnode.take_outbound());
    let tail: Vec<_> = sent[sent.len() - 3..].to_vec();
    assert_eq!(
        tail,
        [
            vec![cmd::ST_ALOCK, 0x0D, 0x16],
            vec![cmd::LT_ALOCK, 0x03, 0xE8],
            vec![cmd::RADIO_STATE, 1],
        ]
    );
}

/// A device may store the limit lossily: 2.09% echoed as 2.08%, 100% as 0 (no limit).
#[test]
fn airtime_lock_echoes_are_recorded_not_compared() {
    for (asked, echoed) in [(209_u16, 208_u16), (10_000, 0)] {
        let config = RNodeConfig {
            st_alock: Some(asked),
            lt_alock: Some(asked),
            ..RNodeConfig::new(params())
        };
        let mut rnode = started(config);
        rnode.on_serial(&device_echoes(&config, |e| {
            let state = e.pop().unwrap();
            for marker in [cmd::ST_ALOCK, cmd::LT_ALOCK] {
                e.push([&[marker][..], &echoed.to_be_bytes()].concat());
            }
            e.push(state);
        }));
        assert_eq!(rnode.take_fault(), None);
        assert!(rnode.is_online());
        assert_eq!(rnode.reported().st_alock, Some(echoed));
        assert_eq!(rnode.reported().lt_alock, Some(echoed));
    }
}

#[test]
fn mismatched_echoes_fault_instead_of_going_online() {
    let config = RNodeConfig::new(params());
    type Tweak = fn(&mut Vec<Vec<u8>>);
    let cases: [(Tweak, Option<Mismatch>); 5] = [
        (|e| e[5][1] -= 1, Some(Mismatch::TxPower)),
        (|e| e[3][4] = e[3][4].wrapping_add(100), None),
        (
            |e| e[3][4] = e[3][4].wrapping_add(101),
            Some(Mismatch::Frequency),
        ),
        (|e| drop(e.remove(3)), Some(Mismatch::Frequency)),
        (|e| e[7][1] = 8, Some(Mismatch::CodingRate)),
    ];
    for (tweak, expected) in cases {
        let mut rnode = started(config);
        rnode.on_serial(&device_echoes(&config, tweak));
        assert_eq!(rnode.take_fault(), expected.map(Fault::Mismatch));
        assert_eq!(rnode.is_online(), expected.is_none());
    }
}

#[test]
fn old_firmware_is_refused() {
    let config = RNodeConfig::new(params());
    let mut rnode = started(config);
    rnode.on_serial(&device_echoes(&config, |e| {
        e[1] = vec![cmd::FW_VERSION, 1, 51]
    }));
    assert_eq!(rnode.take_fault(), Some(Fault::Firmware(1, 51)));
}

#[test]
fn errors_are_classified_as_rns_does() {
    let mut rnode = online(RNodeConfig::new(params()));
    rnode.on_serial(&kiss::encode(&[cmd::ERROR, 0x05]));
    assert_eq!(rnode.take_fault(), None, "memory low is recorded");
    assert_eq!(rnode.take_last_error(), Some(vec![0x05]));
    assert!(rnode.is_online());
    rnode.on_serial(&kiss::encode(&[cmd::ERROR, 0x02]));
    assert_eq!(rnode.take_fault(), Some(Fault::Device(0x02)));
    assert!(!rnode.is_online());
}

#[test]
fn a_reset_while_online_is_a_fault() {
    let mut rnode = started(RNodeConfig::new(params()));
    rnode.on_serial(&kiss::encode(&[cmd::RESET, RESET_MARKER]));
    assert_eq!(rnode.take_fault(), None, "a reset before init is expected");
    let mut rnode = online(RNodeConfig::new(params()));
    rnode.on_serial(&kiss::encode(&[cmd::RESET, RESET_MARKER]));
    assert_eq!(rnode.take_fault(), Some(Fault::Reset));
}

#[test]
fn leave_turns_the_radio_off_then_says_goodbye() {
    let mut rnode = online(RNodeConfig::new(params()));
    rnode.leave();
    assert_eq!(
        rnode.take_outbound(),
        [0xC0, 0x06, 0x00, 0xC0, 0xC0, 0x0A, 0xFF, 0xC0]
    );
    assert!(!rnode.is_online());
}

#[test]
fn flow_control_holds_one_frame_until_ready() {
    let config = RNodeConfig {
        flow_control: true,
        ..RNodeConfig::new(params())
    };
    let mut rnode = online(config);
    rnode.enqueue(b"one").unwrap();
    assert!(matches!(rnode.enqueue(b"two"), Err(ModemError::Busy)));
    rnode.on_serial(&kiss::encode(&[cmd::READY, 0x01]));
    rnode.enqueue(b"two").unwrap();
    assert!(rnode.is_flow_locked());
    rnode.release();
    assert!(rnode.enqueue(b"three").is_ok());
}

#[test]
fn the_hardware_mtu_is_rns_s_508() {
    let mut rnode = online(RNodeConfig::new(params()));
    assert!(rnode.enqueue(&[0xAB; HW_MTU]).is_ok());
    assert!(matches!(
        rnode.enqueue(&[0xAB; HW_MTU + 1]),
        Err(ModemError::TooLong { max: HW_MTU })
    ));
    let mut data = vec![cmd::DATA];
    data.extend([0xC0; HW_MTU]);
    rnode.on_serial(&kiss::encode(&data));
    let received = std::iter::from_fn(|| rnode.poll()).find_map(|event| match event {
        ModemEvent::Received { frame, .. } => Some(frame.len()),
        _ => None,
    });
    assert_eq!(received, Some(HW_MTU));
}

#[test]
fn config_ranges_follow_rns() {
    let ok = RNodeConfig::new(params());
    assert_eq!(ok.validate(), Ok(()));
    let mut bad = ok;
    bad.params.spreading_factor = 13;
    assert_eq!(bad.validate(), Err(ConfigError::SpreadingFactor));
    bad = ok;
    bad.params.frequency_hz = 100_000_000;
    assert_eq!(bad.validate(), Err(ConfigError::Frequency));
    bad = ok;
    bad.st_alock = Some(10_001);
    assert_eq!(bad.validate(), Err(ConfigError::AirtimeLimit));
}
