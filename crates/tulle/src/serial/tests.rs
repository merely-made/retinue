use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::Instant;

use super::*;
use crate::airtime::AirtimeBudget;
use crate::kiss;
use crate::lora::{CodingRate, LoRaParams};
use crate::rnode::cmd;

fn params() -> LoRaParams {
    LoRaParams {
        spreading_factor: 7,
        bandwidth_hz: 125_000,
        coding_rate: CodingRate::Cr45,
        frequency_hz: 915_000_000,
        tx_power_dbm: 7,
        preamble_syms: 8,
        explicit_header: true,
        crc: true,
    }
}

fn config() -> SerialPumpConfig {
    SerialPumpConfig {
        open_settle: Duration::ZERO,
        init_retry: Duration::from_millis(100),
        turnaround: Duration::from_millis(20),
        busy_retry: Duration::from_millis(5),
        ..SerialPumpConfig::default()
    }
}

/// Echo every host frame back verbatim, as the device's log, until the host hangs up.
async fn record_host(mut device: DuplexStream) -> Vec<Vec<u8>> {
    let mut deframer = kiss::Deframer::new(600);
    let mut read = [0u8; 256];
    let mut frames = Vec::new();
    while let Ok(count @ 1..) = device.read(&mut read).await {
        deframer.push(&read[..count], &mut frames);
    }
    frames
}

async fn emulate_rnode(device: DuplexStream) -> Vec<Instant> {
    emulate(device, false).await
}

async fn emulate(mut device: DuplexStream, fail_when_online: bool) -> Vec<Instant> {
    let mut deframer = kiss::Deframer::new(600);
    let mut read = [0u8; 256];
    let mut data_times = Vec::new();
    while let Ok(count @ 1..) = device.read(&mut read).await {
        let mut frames = Vec::new();
        deframer.push(&read[..count], &mut frames);
        for frame in frames {
            let (&command, payload) = frame.split_first().expect("command");
            let response = match command {
                cmd::DETECT => Some(vec![cmd::DETECT, crate::rnode::DETECT_RESP]),
                cmd::FW_VERSION => Some(vec![cmd::FW_VERSION, 1, 86]),
                // Settings are echoed verbatim; the host validates them before going online.
                cmd::FREQUENCY | cmd::BANDWIDTH | cmd::TXPOWER | cmd::SF | cmd::CR => {
                    Some(frame.clone())
                }
                cmd::RADIO_STATE if fail_when_online => {
                    device.write_all(&kiss::encode(&frame)).await.ok();
                    // A transmit failure, which RNS treats as fatal.
                    Some(vec![cmd::ERROR, 0x02])
                }
                cmd::RADIO_STATE => Some(frame.clone()),
                cmd::DATA => {
                    data_times.push(Instant::now());
                    let mut response = vec![cmd::STAT_RSSI, 117]; // -40 dBm
                    device.write_all(&kiss::encode(&response)).await.ok();
                    response = vec![cmd::STAT_SNR, 32]; // 8 dB
                    device.write_all(&kiss::encode(&response)).await.ok();
                    response = vec![cmd::DATA];
                    response.extend_from_slice(payload);
                    Some(response)
                }
                _ => None,
            };
            if let Some(response) = response {
                device.write_all(&kiss::encode(&response)).await.ok();
            }
        }
    }
    data_times
}

#[tokio::test]
async fn initializes_paces_and_delivers_frames() {
    let (host, device) = tokio::io::duplex(4096);
    let emulator = tokio::spawn(emulate_rnode(device));
    let mut pump =
        RNodeSerialLink::spawn_io(host, params(), AirtimeBudget::new(60_000, 1000), config());

    assert_eq!(pump.wait_online().await.unwrap(), Some((1, 86)));
    let first_airtime = pump.send(b"one".to_vec()).await.unwrap();
    let second_airtime = pump.send(b"two".to_vec()).await.unwrap();
    assert_eq!(first_airtime, second_airtime);

    let first = pump.recv().await.expect("first echo");
    let second = pump.recv().await.expect("second echo");
    assert_eq!(first.frame, b"one");
    assert_eq!(second.frame, b"two");
    assert_eq!((first.rssi_dbm, first.snr_db), (-40, 8.0));

    pump.shutdown().await.unwrap();
    let times = emulator.await.unwrap();
    assert_eq!(times.len(), 2);
    assert!(
        times[1].duration_since(times[0]) >= first_airtime + config().turnaround,
        "second frame was not paced by airtime plus turnaround"
    );
}

#[tokio::test]
async fn rejects_a_frame_that_can_never_fit_the_budget() {
    let (host, device) = tokio::io::duplex(4096);
    let emulator = tokio::spawn(emulate_rnode(device));
    let mut pump =
        RNodeSerialLink::spawn_io(host, params(), AirtimeBudget::new(1_000, 1), config());
    pump.wait_online().await.unwrap();
    assert_eq!(
        pump.send(b"cannot fit".to_vec()).await.unwrap_err(),
        TransmitError::DutyCycleImpossible
    );
    pump.shutdown().await.unwrap();
    assert!(emulator.await.unwrap().is_empty());
}

#[tokio::test]
async fn announce_cap_spaces_rnode_egress_from_the_modeled_airtime() {
    let (host, device) = tokio::io::duplex(4096);
    let emulator = tokio::spawn(emulate_rnode(device));
    let budget = AirtimeBudget::new(60_000, 1_000)
        .with_announce_pacing(crate::airtime::AnnouncePacing::Limited { cap_per_mille: 250 });
    let mut pump = RNodeSerialLink::spawn_io(host, params(), budget, config());
    pump.wait_online().await.unwrap();

    let airtime = pump.send_announcement(b"one".to_vec()).await.unwrap();
    pump.send_announcement(b"two".to_vec()).await.unwrap();

    pump.shutdown().await.unwrap();
    let times = emulator.await.unwrap();
    assert_eq!(times.len(), 2);
    assert!(
        times[1].duration_since(times[0]) >= airtime * 4,
        "a 25% cap must keep the second modeled-airtime-sized announce four airtimes away"
    );
}

#[tokio::test]
async fn shutdown_turns_the_radio_off_and_leaves() {
    let (host, device) = tokio::io::duplex(4096);
    let recorder = tokio::spawn(record_host(device));
    let pump =
        RNodeSerialLink::spawn_io(host, params(), AirtimeBudget::new(60_000, 1000), config());
    tokio::time::sleep(Duration::from_millis(50)).await;
    pump.shutdown().await.unwrap();
    let frames = recorder.await.unwrap();
    assert_eq!(
        frames[frames.len() - 2..],
        [vec![cmd::RADIO_STATE, 0x00], vec![cmd::LEAVE, 0xFF]]
    );
}

#[tokio::test]
async fn a_supervised_link_reopens_after_a_fatal_device_error() {
    let (first, first_device) = tokio::io::duplex(4096);
    let (second, second_device) = tokio::io::duplex(4096);
    tokio::spawn(emulate(first_device, true));
    tokio::spawn(emulate_rnode(second_device));
    let mut ports = vec![second, first];
    let mut pump = RNodeSerialLink::spawn(
        move || {
            ports
                .pop()
                .ok_or_else(|| std::io::ErrorKind::NotFound.into())
        },
        crate::rnode::RNodeConfig::new(params()),
        AirtimeBudget::new(60_000, 1000),
        SerialPumpConfig {
            reconnect: Duration::from_millis(200),
            ..config()
        },
        true,
    );
    let mut status = pump.status.clone();
    let fault = status
        .wait_for(|s| matches!(s, PumpStatus::Fault(_)))
        .await
        .unwrap()
        .clone();
    assert!(matches!(fault, PumpStatus::Fault(m) if m.contains("0x02")));
    assert_eq!(
        pump.send(b"while down".to_vec()).await,
        Err(TransmitError::Offline)
    );
    assert_eq!(pump.wait_online().await.unwrap(), Some((1, 86)));
    pump.send(b"after reopen".to_vec()).await.unwrap();
    assert_eq!(pump.recv().await.unwrap().frame, b"after reopen");
    pump.shutdown().await.unwrap();
}
