use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Instant, sleep};

use super::pump::charge_duration_ms;
use super::*;
use crate::airtime::AirtimeBudget;
use crate::lora::{CodingRate, LoRaParams};
use crate::{PhyProfile, WAKE_BYTE, direct_phy};

fn params() -> LoRaParams {
    LoRaParams {
        spreading_factor: 11,
        bandwidth_hz: 250_000,
        coding_rate: CodingRate::Cr45,
        frequency_hz: 906_875_000,
        tx_power_dbm: 17,
        preamble_syms: 16,
        explicit_header: true,
        crc: true,
    }
}

fn profile() -> PhyProfile {
    PhyProfile::meshtastic_long_fast(906_875_000)
}

#[test]
fn charge_duration_ms_preserves_exact_milliseconds_and_rounds_up_fractions() {
    assert_eq!(charge_duration_ms(Duration::from_millis(272)), 272);
    assert_eq!(charge_duration_ms(Duration::from_nanos(272_384_000)), 273);
}

/// With a wake sequence configured, every host command is preceded by the wake preamble —
/// status, configure, and transmit alike. Firmware discards those bytes at a frame
/// boundary, so what remains is exactly the ordinary command stream.
#[tokio::test]
async fn wake_preamble_precedes_every_command() {
    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        transmit_timeout: Duration::from_secs(1),
        wake: Some(WakeSequence {
            preamble: vec![WAKE_BYTE; 4],
            settle: Duration::from_millis(1),
        }),
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 1000);
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);

    let firmware_task = tokio::spawn(async move {
        // Status, wake-prefixed.
        let mut wake = [0_u8; 4];
        firmware.read_exact(&mut wake).await.unwrap();
        assert_eq!(wake, [WAKE_BYTE; 4], "status must be roused first");
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        assert_eq!(&status, b"status\n");
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();

        // Configure, wake-prefixed.
        firmware.read_exact(&mut wake).await.unwrap();
        assert_eq!(wake, [WAKE_BYTE; 4], "configure must be roused first");
        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        assert_eq!(selvage::decode_config_command(&configure), Ok(profile()));
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();

        // Transmit, wake-prefixed, and the command itself is untouched.
        firmware.read_exact(&mut wake).await.unwrap();
        assert_eq!(wake, [WAKE_BYTE; 4], "transmit must be roused first");
        let mut command = [0_u8; 8];
        firmware.read_exact(&mut command).await.unwrap();
        assert_eq!(
            &command, b"\x01\x05\x00hello",
            "the wake bytes must not bleed into the command"
        );
        firmware
            .write_all(&[direct_phy::EVENT_TX, 0, 5, 0])
            .await
            .unwrap();

        // UI snapshot, wake-prefixed and still opaque to Tulle.
        firmware.read_exact(&mut wake).await.unwrap();
        assert_eq!(wake, [WAKE_BYTE; 4], "UI snapshot must be roused first");
        let mut snapshot = [0_u8; 8];
        firmware.read_exact(&mut snapshot).await.unwrap();
        assert_eq!(
            &snapshot,
            &[
                direct_phy::CMD_UI_SNAPSHOT,
                b'0',
                b'1',
                b'0',
                b'2',
                b'0',
                b'3',
                0
            ]
        );
        firmware
            .write_all(&[direct_phy::EVENT_UI_SNAPSHOT, 0])
            .await
            .unwrap();
        sleep(Duration::from_millis(200)).await;
    });

    let ui = link.ui_control();
    link.wait_online().await.unwrap();
    link.send(b"hello".to_vec()).await.unwrap();
    ui.publish(&[1, 2, 3]).await.unwrap();
    link.shutdown().await.unwrap();
    firmware_task.await.unwrap();
}

/// The USB personality is the default and sends no wake bytes, so it pays nothing for a
/// mechanism it does not need.
#[tokio::test]
async fn usb_configuration_sends_no_wake_prefix() {
    assert_eq!(DirectPhySerialConfig::default().wake, None);
    assert!(DirectPhySerialConfig::low_power_uart().wake.is_some());

    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 1000);
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);

    let firmware_task = tokio::spawn(async move {
        // The very first bytes are the command, with nothing in front of them.
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        assert_eq!(&status, b"status\n");
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();
        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();
        sleep(Duration::from_millis(200)).await;
    });

    link.wait_online().await.unwrap();
    link.shutdown().await.unwrap();
    firmware_task.await.unwrap();
}

#[tokio::test]
async fn live_reconfiguration_updates_the_profile_without_reopening_the_link() {
    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        transmit_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 60_000);
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);
    let mut fast = profile();
    fast.spreading_factor = 9;

    let firmware_task = tokio::spawn(async move {
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();

        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        assert_eq!(selvage::decode_config_command(&configure), Ok(profile()));
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();

        firmware.read_exact(&mut configure).await.unwrap();
        assert_eq!(selvage::decode_config_command(&configure), Ok(fast));
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();

        let mut transmit = [0_u8; 8];
        firmware.read_exact(&mut transmit).await.unwrap();
        assert_eq!(&transmit, b"\x01\x05\x00hello");
        firmware
            .write_all(&[direct_phy::EVENT_TX, 0, 5, 0])
            .await
            .unwrap();
        sleep(Duration::from_millis(200)).await;
    });

    link.wait_online().await.unwrap();
    link.reconfigure(fast).await.unwrap();
    let airtime = link.send(b"hello".to_vec()).await.unwrap();
    assert_eq!(airtime, LoRaParams::try_from(fast).unwrap().time_on_air(5));
    link.shutdown().await.unwrap();
    firmware_task.await.unwrap();
}

#[tokio::test]
async fn truncated_snapshot_timeout_resynchronizes_following_transmit() {
    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        transmit_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 1000);
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);

    let firmware_task = tokio::spawn(async move {
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();
        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();

        let mut snapshot = [0_u8; 8];
        firmware.read_exact(&mut snapshot).await.unwrap();
        assert_eq!(snapshot[0], direct_phy::CMD_UI_SNAPSHOT);
        assert_eq!(snapshot[7], WAKE_BYTE);
        // Simulate an interrupted outer frame by withholding its acknowledgement.

        let mut resync = [0_u8; 1];
        firmware.read_exact(&mut resync).await.unwrap();
        assert_eq!(resync, [WAKE_BYTE]);
        let mut transmit = [0_u8; 8];
        firmware.read_exact(&mut transmit).await.unwrap();
        assert_eq!(&transmit, b"\x01\x05\x00hello");
        firmware
            .write_all(&[direct_phy::EVENT_TX, 0, 5, 0])
            .await
            .unwrap();
        sleep(Duration::from_millis(100)).await;
    });

    link.wait_online().await.unwrap();
    assert_eq!(
        link.publish_ui_snapshot(&[1, 2, 3]).await,
        Err(UiSnapshotError::TimedOut)
    );
    link.send(b"hello".to_vec()).await.unwrap();
    link.shutdown().await.unwrap();
    firmware_task.await.unwrap();
}

/// A firmware event split across reads still decodes: the host decoder reassembles rather
/// than requiring each event to arrive whole.
#[tokio::test]
async fn fragmented_firmware_events_reassemble() {
    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 1000);
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);

    let firmware_task = tokio::spawn(async move {
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();
        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();

        // One RX event, dribbled out a byte at a time.
        for byte in [direct_phy::EVENT_RX, 3, 0, 0xd8, 0xff, 9, 0, 7, 8, 9] {
            firmware.write_all(&[byte]).await.unwrap();
            sleep(Duration::from_millis(2)).await;
        }
        sleep(Duration::from_millis(200)).await;
    });

    link.wait_online().await.unwrap();
    let received = link.recv().await.unwrap();
    assert_eq!(received.frame, [7, 8, 9], "a split event still reassembles");
    assert_eq!(received.rssi_dbm, -40);
    link.shutdown().await.unwrap();
    firmware_task.await.unwrap();
}

#[tokio::test]
async fn pump_sends_and_receives_complete_frames() {
    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        transmit_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 1000);
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);

    let firmware_task = tokio::spawn(async move {
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        assert_eq!(&status, b"status\n");
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();

        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        assert_eq!(selvage::decode_config_command(&configure), Ok(profile()));
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();

        let mut command = [0_u8; 8];
        firmware.read_exact(&mut command).await.unwrap();
        assert_eq!(&command, b"\x01\x05\x00hello");
        firmware
            .write_all(&[direct_phy::EVENT_RX, 3, 0, 0xd8, 0xff, 9, 0, 7, 8, 9])
            .await
            .unwrap();
        firmware
            .write_all(&[direct_phy::EVENT_TX, 0, 5, 0])
            .await
            .unwrap();
        sleep(Duration::from_secs(1)).await;
    });

    link.wait_online().await.unwrap();
    let airtime = link.send(b"hello".to_vec()).await.unwrap();
    assert_eq!(airtime, params().time_on_air(5));
    let received = link.recv().await.unwrap();
    assert_eq!(received.frame, [7, 8, 9]);
    assert_eq!(received.rssi_dbm, -40);
    assert_eq!(received.snr_db, 9.0);
    link.shutdown().await.unwrap();
    firmware_task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn announce_cap_spaces_direct_phy_egress_from_the_modeled_airtime() {
    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        transmit_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 1_000)
        .with_announce_pacing(crate::airtime::AnnouncePacing::Limited { cap_per_mille: 250 });
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);

    let firmware_task = tokio::spawn(async move {
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();
        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();

        let mut sent_at = Vec::new();
        for expected in [b"one", b"two"] {
            let mut command = [0_u8; 6];
            firmware.read_exact(&mut command).await.unwrap();
            assert_eq!(command[0], direct_phy::CMD_TX);
            assert_eq!(&command[3..], expected);
            sent_at.push(Instant::now());
            firmware
                .write_all(&[direct_phy::EVENT_TX, 0, 3, 0])
                .await
                .unwrap();
        }
        // Keep the device end open until the host's orderly shutdown.
        sleep(Duration::from_millis(200)).await;
        sent_at
    });

    link.wait_online().await.unwrap();
    let airtime = link.send_announcement(b"one".to_vec()).await.unwrap();
    assert_eq!(airtime, params().time_on_air(3));
    let modeled_cooldown = params().time_on_air(3).saturating_mul(4);
    let charged_cooldown = Duration::from_millis(charge_duration_ms(params().time_on_air(3)) * 4);
    {
        let second = link.send_announcement(b"two".to_vec());
        tokio::pin!(second);
        tokio::select! {
            biased;
            result = &mut second => panic!("second announce completed without pacing: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        tokio::time::advance(charged_cooldown - Duration::from_millis(1)).await;
        tokio::select! {
            result = &mut second => panic!("second announce escaped the modeled pacing gate: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        second.await.unwrap();
    }
    link.shutdown().await.unwrap();
    tokio::time::advance(Duration::from_millis(200)).await;
    let times = firmware_task.await.unwrap();
    assert!(
        times[1].duration_since(times[0]) >= modeled_cooldown,
        "a 25% cap must keep the second modeled-airtime-sized announce four airtimes away"
    );
    assert!(
        times[1].duration_since(times[0]) >= charged_cooldown,
        "the millisecond budget must charge fractional modeled airtime upward"
    );
}

#[tokio::test]
async fn pump_retries_status_and_profile_during_startup() {
    let (host, mut firmware) = tokio::io::duplex(2048);
    let config = DirectPhySerialConfig {
        open_settle: Duration::ZERO,
        online_timeout: Duration::from_secs(2),
        ..Default::default()
    };
    let budget = AirtimeBudget::new(60_000, 1000);
    let mut link = DirectPhySerialLink::spawn_io(host, profile(), params(), budget, config);

    let firmware_task = tokio::spawn(async move {
        let mut status = [0_u8; 7];
        firmware.read_exact(&mut status).await.unwrap();
        assert_eq!(&status, b"status\n");
        let mut configure = [0_u8; selvage::CONFIG_COMMAND_LEN];
        firmware.read_exact(&mut configure).await.unwrap();
        assert_eq!(selvage::decode_config_command(&configure), Ok(profile()));

        firmware.read_exact(&mut status).await.unwrap();
        assert_eq!(&status, b"status\n");
        firmware
            .write_all(b"tulle/test phy online\r\n")
            .await
            .unwrap();
        firmware.read_exact(&mut configure).await.unwrap();
        assert_eq!(selvage::decode_config_command(&configure), Ok(profile()));
        firmware
            .write_all(&[direct_phy::EVENT_CONFIG, 0])
            .await
            .unwrap();
        sleep(Duration::from_secs(1)).await;
    });

    link.wait_online().await.unwrap();
    link.shutdown().await.unwrap();
    firmware_task.await.unwrap();
}
