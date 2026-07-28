#![no_std]
#![no_main]

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::Config;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::spi::{
    Mode,
    master::{Config as SpiConfig, Spi},
};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
#[cfg(feature = "host-uart-low-power")]
use esp_hal::uart::{Config as UartConfig, Uart};
#[cfg(feature = "host-usb")]
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use lora_modulation::{Bandwidth, CodingRate, SpreadingFactor};
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{Config as Sx126xConfig, Sx126x, Sx1262, TcxoCtrlVoltage};
use lora_phy::{LoRa, RxMode};
use selvage::{
    CMD_CONFIG, CMD_TX, CONFIG_COMMAND_LEN, EVENT_CONFIG, EVENT_DIAGNOSTIC, EVENT_RX, EVENT_TX,
    MESHTASTIC_SYNC_WORD, WAKE_BYTE, decode_config_command,
};

mod board;
mod power;
mod ui;

esp_bootloader_esp_idf::esp_app_desc!();

/// Host UART line rate for the low-power personality. Matches the host default in
/// `tulle::direct_phy_serial`.
#[cfg(feature = "host-uart-low-power")]
const HOST_UART_BAUD: u32 = 115_200;

const MAX_RADIO_FRAME: usize = 255;

fn spreading_factor(value: u8) -> Option<SpreadingFactor> {
    Some(match value {
        5 => SpreadingFactor::_5,
        6 => SpreadingFactor::_6,
        7 => SpreadingFactor::_7,
        8 => SpreadingFactor::_8,
        9 => SpreadingFactor::_9,
        10 => SpreadingFactor::_10,
        11 => SpreadingFactor::_11,
        12 => SpreadingFactor::_12,
        _ => return None,
    })
}

fn bandwidth(value: u32) -> Option<Bandwidth> {
    Some(match value {
        7_810 => Bandwidth::_7KHz,
        10_420 => Bandwidth::_10KHz,
        15_630 => Bandwidth::_15KHz,
        20_830 => Bandwidth::_20KHz,
        31_250 => Bandwidth::_31KHz,
        41_670 => Bandwidth::_41KHz,
        62_500 => Bandwidth::_62KHz,
        125_000 => Bandwidth::_125KHz,
        250_000 => Bandwidth::_250KHz,
        500_000 => Bandwidth::_500KHz,
        _ => return None,
    })
}

fn coding_rate(value: u8) -> Option<CodingRate> {
    Some(match value {
        5 => CodingRate::_4_5,
        6 => CodingRate::_4_6,
        7 => CodingRate::_4_7,
        8 => CodingRate::_4_8,
        _ => return None,
    })
}

/// Write to whichever host link this build selected. Generic over the transport so the USB
/// and UART personalities share one protocol implementation rather than drifting apart.
async fn write_all<W: embedded_io_async::Write>(tx: &mut W, bytes: &[u8]) -> bool {
    embedded_io_async::Write::write_all(tx, bytes).await.is_ok()
        && embedded_io_async::Write::flush(tx).await.is_ok()
}

async fn serve_status_only<R: embedded_io_async::Read, W: embedded_io_async::Write>(
    mut rx: R,
    mut tx: W,
    status: &'static [u8],
) -> ! {
    let _ = write_all(&mut tx, status).await;
    let mut buffer = [0_u8; 64];
    loop {
        match embedded_io_async::Read::read(&mut rx, &mut buffer).await {
            Ok(length) if length > 0 => {
                let reply = if &buffer[..length] == b"sync\n" || &buffer[..length] == b"sync\r\n" {
                    b"2b 24b4\r\n".as_slice()
                } else {
                    status
                };
                let _ = write_all(&mut tx, reply).await;
            }
            _ => {}
        }
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(Config::default());

    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);

    // The USB personality keeps the stock idle loop: its host link cannot survive Light-sleep,
    // so there is nothing to gain and a re-enumeration failure to lose.
    #[cfg(feature = "host-usb")]
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // The low-power personality installs a gated idle hook instead. It only sleeps once
    // `power::arm` hands it the RTC, which happens after the radio is receiving.
    #[cfg(feature = "host-uart-low-power")]
    esp_rtos::start_with_idle_hook(timg0.timer0, sw_int.software_interrupt0, power::idle);

    // The host link. Both personalities implement `embedded_io_async::{Read, Write}`, so
    // everything below is written once and built twice.
    #[cfg(feature = "host-usb")]
    let (mut usb_rx, mut usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
        .into_async()
        .split();

    // UART0 on the exposed header: GPIO44 RX, GPIO43 TX. Unlike USB Serial/JTAG this survives
    // Light-sleep and can wake the chip, which is what the low-power personality needs.
    #[cfg(feature = "host-uart-low-power")]
    let (mut usb_rx, mut usb_tx) = {
        let uart = Uart::new(
            peripherals.UART0,
            UartConfig::default().with_baudrate(HOST_UART_BAUD),
        )
        .unwrap()
        .with_rx(peripherals.GPIO44)
        .with_tx(peripherals.GPIO43)
        .into_async();
        uart.split()
    };

    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO17)
    .with_scl(peripherals.GPIO18)
    .into_async();
    let button = Input::new(
        peripherals.GPIO0,
        InputConfig::default().with_pull(Pull::Up),
    );
    let oled_reset = Output::new(peripherals.GPIO21, Level::High, OutputConfig::default());
    let vext = Output::new(peripherals.GPIO36, Level::Low, OutputConfig::default());
    let led = Output::new(peripherals.GPIO35, Level::Low, OutputConfig::default());
    let mut local_status = board::initial_status();
    spawner.spawn(ui::button_task(button).unwrap());
    spawner.spawn(ui::screen_task(i2c, oled_reset, vext, led, local_status).unwrap());

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(1))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO9)
    .with_mosi(peripherals.GPIO10)
    .with_miso(peripherals.GPIO11)
    .into_async();
    let cs = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
    let spi = ExclusiveDevice::new(spi, cs, Delay).unwrap();

    let reset = Output::new(peripherals.GPIO12, Level::High, OutputConfig::default());
    let busy = Input::new(peripherals.GPIO13, InputConfig::default());
    let dio1 = Input::new(peripherals.GPIO14, InputConfig::default());
    let interface = GenericSx126xInterfaceVariant::new(reset, dio1, busy, None, None).unwrap();
    let radio = Sx126x::new(
        spi,
        interface,
        Sx126xConfig {
            chip: Sx1262,
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: true,
            rx_boost: true,
        },
    );
    let mut lora = match LoRa::new_with_sync_word(radio, MESHTASTIC_SYNC_WORD, Delay).await {
        Ok(lora) => lora,
        Err(_) => {
            local_status.radio = radio_face::RadioState::Fault;
            local_status.fault = Some(radio_face::Fault {
                code: 1,
                message: radio_face::Text::from_truncated("SX1262 INIT"),
            });
            ui::publish(local_status, radio_face::LedSignal::Idle);
            serve_status_only(
                usb_rx,
                usb_tx,
                b"tulle/heltec-v4 phy online; sx1262 init failed\r\n",
            )
            .await
        }
    };

    let mut modulation = match lora.create_modulation_params(
        SpreadingFactor::_11,
        Bandwidth::_250KHz,
        CodingRate::_4_5,
        board::DEFAULT_FREQUENCY_HZ,
    ) {
        Ok(params) => params,
        Err(_) => {
            local_status.radio = radio_face::RadioState::Fault;
            local_status.fault = Some(radio_face::Fault {
                code: 2,
                message: radio_face::Text::from_truncated("PHY PARAMS"),
            });
            ui::publish(local_status, radio_face::LedSignal::Idle);
            serve_status_only(
                usb_rx,
                usb_tx,
                b"tulle/heltec-v4 phy modulation invalid\r\n",
            )
            .await
        }
    };
    let mut tx_params = match lora.create_tx_packet_params(16, false, true, false, &modulation) {
        Ok(params) => params,
        Err(_) => {
            local_status.radio = radio_face::RadioState::Fault;
            local_status.fault = Some(radio_face::Fault {
                code: 3,
                message: radio_face::Text::from_truncated("TX PARAMS"),
            });
            ui::publish(local_status, radio_face::LedSignal::Idle);
            serve_status_only(
                usb_rx,
                usb_tx,
                b"tulle/heltec-v4 phy tx parameters invalid\r\n",
            )
            .await
        }
    };
    let mut rx_params = match lora.create_rx_packet_params(16, false, 255, true, false, &modulation)
    {
        Ok(params) => params,
        Err(_) => {
            local_status.radio = radio_face::RadioState::Fault;
            local_status.fault = Some(radio_face::Fault {
                code: 4,
                message: radio_face::Text::from_truncated("RX PARAMS"),
            });
            ui::publish(local_status, radio_face::LedSignal::Idle);
            serve_status_only(
                usb_rx,
                usb_tx,
                b"tulle/heltec-v4 phy rx parameters invalid\r\n",
            )
            .await
        }
    };

    let online =
        b"tulle/heltec-v4 phy online; sx1262 online; sync=2b reg=24b4; longfast=906875000\r\n";
    local_status.radio = radio_face::RadioState::Online;
    local_status.fault = None;
    ui::publish(local_status, radio_face::LedSignal::Idle);
    let _ = write_all(&mut usb_tx, online).await;
    let mut usb_command = [0_u8; 3 + MAX_RADIO_FRAME];
    let mut usb_command_len = 0_usize;
    let mut prepare_rx = true;
    let mut tx_power_dbm = i32::from(board::DEFAULT_TX_POWER_DBM);

    // Hand the RTC to the idle hook. Only now is sleeping meaningful: the radio is about to be
    // armed for continuous receive, so a sleeping CPU still hears packets.
    #[cfg(feature = "host-uart-low-power")]
    power::arm(esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR));

    loop {
        if prepare_rx {
            // Configuring the radio is SPI traffic; sleeping through it would abandon a
            // half-finished transaction.
            let _awake = power::Awake::new();
            if lora
                .prepare_for_rx(RxMode::Continuous, &modulation, &rx_params)
                .await
                .is_err()
            {
                local_status.radio = radio_face::RadioState::Fault;
                local_status.fault = Some(radio_face::Fault {
                    code: 5,
                    message: radio_face::Text::from_truncated("RX SETUP"),
                });
                ui::publish(local_status, radio_face::LedSignal::Idle);
                let _ = write_all(&mut usb_tx, b"radio rx setup failed\r\n").await;
                continue;
            }
            local_status.radio = radio_face::RadioState::Online;
            local_status.fault = None;
            ui::publish(local_status, radio_face::LedSignal::Idle);
            prepare_rx = false;
        }

        let mut usb_packet = [0_u8; 64];
        let mut radio_frame = [0_u8; MAX_RADIO_FRAME];
        // The one point this loop is genuinely idle: both sides are merely waiting, the radio
        // is listening on its own, and nothing is half-done. Sleeping is allowed here and
        // nowhere else — but only at a frame boundary, since a wake eats the bytes that
        // triggered it and would truncate a command already in progress.
        let waiting = select(
            embedded_io_async::Read::read(&mut usb_rx, &mut usb_packet),
            lora.rx(&rx_params, &mut radio_frame),
        );
        let outcome = if usb_command_len == 0 {
            waiting.await
        } else {
            let _awake = power::Awake::new();
            waiting.await
        };
        // Everything past here touches SPI, the radio, or the host link.
        let _awake = power::Awake::new();
        match outcome {
            Either::Second(Ok((length, packet_status))) => {
                let length = usize::from(length);
                let mut event = [0_u8; 7 + MAX_RADIO_FRAME];
                event[0] = EVENT_RX;
                event[1..3].copy_from_slice(&(length as u16).to_le_bytes());
                event[3..5].copy_from_slice(&packet_status.rssi.to_le_bytes());
                event[5..7].copy_from_slice(&packet_status.snr.to_le_bytes());
                event[7..7 + length].copy_from_slice(&radio_frame[..length]);
                let _ = write_all(&mut usb_tx, &event[..7 + length]).await;
                local_status.rx_frames = local_status.rx_frames.saturating_add(1);
                local_status.last_rx = Some(radio_face::RxSummary {
                    frame_len: length as u16,
                    rssi_dbm: packet_status.rssi,
                    snr_tenths_db: packet_status.snr.saturating_mul(10),
                });
                local_status.last_wake = radio_face::WakeSource::Radio;
                ui::publish(local_status, radio_face::LedSignal::Activity);
            }
            Either::Second(Err(_)) => {
                local_status.radio = radio_face::RadioState::Fault;
                local_status.fault = Some(radio_face::Fault {
                    code: 6,
                    message: radio_face::Text::from_truncated("RADIO RX"),
                });
                ui::publish(local_status, radio_face::LedSignal::Idle);
                let _ = write_all(&mut usb_tx, b"radio rx failed\r\n").await;
                prepare_rx = true;
            }
            Either::First(Err(_)) => {}
            Either::First(Ok(0)) => {}
            Either::First(Ok(length)) => {
                local_status.host = radio_face::HostState::Attached;
                local_status.last_wake = radio_face::WakeSource::Host;
                ui::publish(local_status, radio_face::LedSignal::Idle);
                let mut packet = &usb_packet[..length];
                // Discard host wake bytes, but only while the parser sits at a frame
                // boundary: the same value is perfectly legal inside a length field or a
                // payload, so stripping it anywhere else would corrupt the frame.
                if usb_command_len == 0 {
                    let skip = packet
                        .iter()
                        .position(|byte| *byte != WAKE_BYTE)
                        .unwrap_or(packet.len());
                    packet = &packet[skip..];
                    if packet.is_empty() {
                        continue;
                    }
                }
                let length = packet.len();
                if packet == b"status\n" || packet == b"status\r\n" {
                    let _ = write_all(&mut usb_tx, online).await;
                    continue;
                }
                if packet == b"sync\n" || packet == b"sync\r\n" {
                    let _ = write_all(&mut usb_tx, b"2b 24b4\r\n").await;
                    continue;
                }
                if packet == b"ui\n" || packet == b"ui\r\n" {
                    let diagnostic = ui::diagnostic();
                    let mut reply = radio_face::Text::<80>::empty();
                    let _ = write!(
                        &mut reply,
                        "ui={}; display={}; screen={}; button={}\r\n",
                        diagnostic.state, diagnostic.display, diagnostic.screen, diagnostic.button,
                    );
                    let _ = write_all(&mut usb_tx, reply.as_str().as_bytes()).await;
                    continue;
                }
                #[cfg(feature = "ui-bench")]
                if packet == b"fault\n" || packet == b"fault\r\n" {
                    local_status.radio = radio_face::RadioState::Fault;
                    local_status.fault = Some(radio_face::Fault {
                        code: 0xfe,
                        message: radio_face::Text::from_truncated("BENCH FAULT"),
                    });
                    ui::publish(local_status, radio_face::LedSignal::Idle);
                    let _ = write_all(&mut usb_tx, b"ui bench fault set\r\n").await;
                    continue;
                }
                #[cfg(feature = "ui-bench")]
                if packet == b"clear\n" || packet == b"clear\r\n" {
                    local_status.radio = radio_face::RadioState::Online;
                    local_status.fault = None;
                    ui::publish(local_status, radio_face::LedSignal::Idle);
                    let _ = write_all(&mut usb_tx, b"ui bench fault cleared\r\n").await;
                    continue;
                }
                // Sleep diagnostics, for the power receipt: how many times the idle hook
                // actually slept, and how many times it wanted to but the gate was closed.
                // A build that never sleeps answers with zeros rather than nothing, so the
                // bench can tell "not sleeping" from "wrong firmware".
                if packet == b"sleep\n" || packet == b"sleep\r\n" {
                    let (entries, blocked) = power::counters();
                    let mut report = [0_u8; 9];
                    report[0] = EVENT_DIAGNOSTIC;
                    report[1..5].copy_from_slice(&entries.to_le_bytes());
                    report[5..9].copy_from_slice(&blocked.to_le_bytes());
                    let _ = write_all(&mut usb_tx, &report).await;
                    continue;
                }

                if usb_command_len + length > usb_command.len() {
                    usb_command_len = 0;
                    let _ = write_all(&mut usb_tx, &[EVENT_TX, 2, 0, 0]).await;
                    continue;
                }
                usb_command[usb_command_len..usb_command_len + length].copy_from_slice(packet);
                usb_command_len += length;

                let command_len = match usb_command.first().copied() {
                    Some(CMD_TX) if usb_command_len >= 3 => {
                        3 + usize::from(u16::from_le_bytes([usb_command[1], usb_command[2]]))
                    }
                    Some(CMD_CONFIG) => CONFIG_COMMAND_LEN,
                    Some(_) if usb_command_len >= 1 => {
                        usb_command_len = 0;
                        let _ = write_all(&mut usb_tx, &[EVENT_TX, 3, 0, 0]).await;
                        continue;
                    }
                    _ => continue,
                };
                if usb_command_len < command_len {
                    continue;
                }

                if usb_command[0] == CMD_CONFIG {
                    let result = match decode_config_command(&usb_command[..CONFIG_COMMAND_LEN]) {
                        Ok(profile) => {
                            let radio_params = spreading_factor(profile.spreading_factor)
                                .zip(bandwidth(profile.bandwidth_hz))
                                .zip(coding_rate(profile.coding_rate_denominator));
                            match radio_params {
                                Some(((sf, bw), cr)) => {
                                    match lora.create_modulation_params(
                                        sf,
                                        bw,
                                        cr,
                                        profile.frequency_hz,
                                    ) {
                                        Ok(new_modulation) => {
                                            let new_tx = lora.create_tx_packet_params(
                                                profile.preamble_symbols,
                                                !profile.explicit_header,
                                                profile.crc,
                                                profile.invert_iq,
                                                &new_modulation,
                                            );
                                            let new_rx = lora.create_rx_packet_params(
                                                profile.preamble_symbols,
                                                !profile.explicit_header,
                                                255,
                                                profile.crc,
                                                profile.invert_iq,
                                                &new_modulation,
                                            );
                                            match (new_tx, new_rx) {
                                                (Ok(new_tx), Ok(new_rx)) => {
                                                    if lora
                                                        .set_sync_word(profile.sync_word)
                                                        .await
                                                        .is_ok()
                                                    {
                                                        modulation = new_modulation;
                                                        tx_params = new_tx;
                                                        rx_params = new_rx;
                                                        tx_power_dbm =
                                                            i32::from(profile.tx_power_dbm);
                                                        board::apply_profile(
                                                            &mut local_status,
                                                            profile,
                                                        );
                                                        prepare_rx = true;
                                                        0
                                                    } else {
                                                        3
                                                    }
                                                }
                                                _ => 2,
                                            }
                                        }
                                        Err(_) => 2,
                                    }
                                }
                                None => 2,
                            }
                        }
                        Err(_) => 1,
                    };
                    usb_command_len = 0;
                    let _ = write_all(&mut usb_tx, &[EVENT_CONFIG, result]).await;
                    continue;
                }

                let frame_len = command_len - 3;
                if frame_len > MAX_RADIO_FRAME {
                    usb_command_len = 0;
                    let _ = write_all(&mut usb_tx, &[EVENT_TX, 4, 0, 0]).await;
                    continue;
                }

                let sent_len = frame_len as u16;
                let result = if lora
                    .prepare_for_tx(
                        &modulation,
                        &mut tx_params,
                        tx_power_dbm,
                        &usb_command[3..3 + frame_len],
                    )
                    .await
                    .is_ok()
                    && lora.tx().await.is_ok()
                {
                    0
                } else {
                    1
                };
                usb_command_len = 0;
                prepare_rx = true;
                let length_bytes = sent_len.to_le_bytes();
                let _ = write_all(
                    &mut usb_tx,
                    &[EVENT_TX, result, length_bytes[0], length_bytes[1]],
                )
                .await;
                if result == 0 {
                    local_status.tx_frames = local_status.tx_frames.saturating_add(1);
                    local_status.last_tx = radio_face::TxResult::Sent {
                        frame_len: sent_len,
                    };
                } else {
                    local_status.last_tx = radio_face::TxResult::Failed { code: result };
                }
                ui::publish(local_status, radio_face::LedSignal::Activity);
            }
        }
    }
}
