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

async fn emulate_rnode(mut device: DuplexStream) -> Vec<Instant> {
    let mut deframer = kiss::Deframer::new(600);
    let mut read = [0u8; 256];
    let mut data_times = Vec::new();
    loop {
        let count = device.read(&mut read).await.expect("host read");
        if count == 0 {
            break;
        }
        let mut frames = Vec::new();
        deframer.push(&read[..count], &mut frames);
        for frame in frames {
            let (&command, payload) = frame.split_first().expect("command");
            let response = match command {
                cmd::DETECT => Some(vec![cmd::DETECT, crate::rnode::DETECT_RESP]),
                cmd::FW_VERSION => Some(vec![cmd::FW_VERSION, 1, 86]),
                cmd::RADIO_STATE => Some(vec![cmd::RADIO_STATE, 1]),
                cmd::DATA => {
                    data_times.push(Instant::now());
                    let mut response = vec![cmd::STAT_RSSI, 117]; // -40 dBm
                    device.write_all(&kiss::encode(&response)).await.unwrap();
                    response = vec![cmd::STAT_SNR, 32]; // 8 dB
                    device.write_all(&kiss::encode(&response)).await.unwrap();
                    response = vec![cmd::DATA];
                    response.extend_from_slice(payload);
                    Some(response)
                }
                _ => None,
            };
            if let Some(response) = response {
                device.write_all(&kiss::encode(&response)).await.unwrap();
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
