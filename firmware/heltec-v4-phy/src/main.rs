#![no_std]
#![no_main]

use core::fmt::Write as _;
#[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
use core::future::{Future, poll_fn};
#[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
use core::task::Poll;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::Config;
#[cfg(feature = "rf-sleep-proof")]
use esp_hal::delay::Delay as BlockingDelay;
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
use radio_hand::dispatch;
use radio_hand::executive::{ChipDiagnostics, Executive, Face, NoStore, RadioState};
use radio_hand::link::HostLink;
use radio_hand::region::Region;
use selvage::{
    CommandStream, EVENT_DIAGNOSTIC, EVENT_RX, MAX_COMMAND_LEN, MESHTASTIC_SYNC_WORD, WAKE_BYTE,
};

mod board;
mod host;
mod power;
mod ui;
mod wake_input;

esp_bootloader_esp_idf::esp_app_desc!();

/// Host UART line rate for the low-power personality. Matches the host default in
/// `tulle::direct_phy_serial`.
#[cfg(feature = "host-uart-low-power")]
const HOST_UART_BAUD: u32 = 115_200;

const MAX_RADIO_FRAME: usize = 255;

#[cfg(feature = "rf-sleep-proof")]
const RF_SLEEP_PROOF_CHALLENGE: &[u8; 21] = b"tulle/sleep-proof/v1?";
#[cfg(feature = "rf-sleep-proof")]
const RF_SLEEP_PROOF_RECEIPT: &[u8; 21] = b"tulle/sleep-proof/v1!";

#[cfg(feature = "rf-sleep-proof")]
fn sleep_proof_nonce(frame: &[u8]) -> Option<u32> {
    let nonce = frame.strip_prefix(RF_SLEEP_PROOF_CHALLENGE)?;
    let nonce: [u8; 4] = nonce.try_into().ok()?;
    Some(u32::from_le_bytes(nonce))
}

#[cfg(feature = "rf-sleep-proof")]
fn sleep_proof_receipt(
    nonce: u32,
    sleep_entries: u32,
    wake_registrations: u32,
    received_frames: u32,
    last_sleep_us: u32,
    sleep_enabled: bool,
    reset_reason: u32,
) -> [u8; 49] {
    let mut receipt = [0_u8; 49];
    receipt[..RF_SLEEP_PROOF_RECEIPT.len()].copy_from_slice(RF_SLEEP_PROOF_RECEIPT);
    receipt[21..25].copy_from_slice(&nonce.to_le_bytes());
    receipt[25..29].copy_from_slice(&sleep_entries.to_le_bytes());
    receipt[29..33].copy_from_slice(&wake_registrations.to_le_bytes());
    receipt[33..37].copy_from_slice(&received_frames.to_le_bytes());
    receipt[37..41].copy_from_slice(&last_sleep_us.to_le_bytes());
    receipt[41..45].copy_from_slice(&u32::from(sleep_enabled).to_le_bytes());
    receipt[45..49].copy_from_slice(&reset_reason.to_le_bytes());
    receipt
}

/// The board's own SX1262 registers, for `radio-hand`'s dispatch.
///
/// Chip-specific, so the shared loop reaches it through [`ChipDiagnostics`]. The V4 had no
/// diagnostics path before: its transmit could not time out, so nothing ever asked.
struct Sx126xDiagnostics;

impl<SPI, IV, C, DLY> ChipDiagnostics<Sx126x<SPI, IV, C>, DLY> for Sx126xDiagnostics
where
    SPI: embedded_hal_async::spi::SpiDevice<u8>,
    IV: lora_phy::mod_traits::InterfaceVariant,
    C: lora_phy::sx126x::Sx126xVariant,
    DLY: lora_phy::DelayNs,
{
    async fn read(&self, lora: &mut LoRa<Sx126x<SPI, IV, C>, DLY>) -> [u8; 7] {
        match lora.sx126x_diagnostics().await {
            Ok(d) => {
                let irq = d.irq_status.to_le_bytes();
                let errors = d.device_errors.to_le_bytes();
                [
                    EVENT_DIAGNOSTIC,
                    irq[0],
                    irq[1],
                    errors[0],
                    errors[1],
                    d.sync_word[0],
                    d.sync_word[1],
                ]
            }
            Err(_) => [EVENT_DIAGNOSTIC, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        }
    }
}

/// Write to whichever host link this build selected. Generic over the transport so the USB
/// and UART personalities share one protocol implementation rather than drifting apart.
async fn write_all<W: embedded_io_async::Write>(tx: &mut W, bytes: &[u8]) -> bool {
    embedded_io_async::Write::write_all(tx, bytes).await.is_ok()
        && embedded_io_async::Write::flush(tx).await.is_ok()
}

#[cfg(feature = "rf-sleep-proof")]
async fn ignore_host<R: embedded_io_async::Read>(
    _rx: &mut R,
    _buffer: &mut [u8],
) -> Result<usize, R::Error> {
    core::future::pending().await
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
    #[cfg(feature = "rf-sleep-proof")]
    let proof_reset_reason = esp_hal::system::reset_reason()
        .map(|reason| reason as u32)
        .unwrap_or_default();

    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);

    // The USB personality keeps the stock idle loop: its host link cannot survive Light-sleep,
    // so there is nothing to gain and a re-enumeration failure to lose.
    #[cfg(any(feature = "host-usb", feature = "rf-sleep-proof"))]
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // The low-power personality installs a gated idle hook instead. It only sleeps once
    // `power::arm` hands it the RTC, which happens after the radio is receiving.
    #[cfg(all(feature = "host-uart-low-power", not(feature = "rf-sleep-proof")))]
    esp_rtos::start_with_idle_hook(timg0.timer0, sw_int.software_interrupt0, power::idle);

    // The host link. Both personalities implement `embedded_io_async::{Read, Write}`, so
    // everything below is written once and built twice.
    #[cfg(feature = "host-usb")]
    let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
        .into_async()
        .split();

    // UART0 on the exposed header: GPIO44 RX, GPIO43 TX. Unlike USB Serial/JTAG this survives
    // Light-sleep and can wake the chip, which is what the low-power personality needs.
    #[cfg(feature = "host-uart-low-power")]
    let (usb_rx, usb_tx) = {
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
    let busy = wake_input::V4Input::new(
        Input::new(peripherals.GPIO13, InputConfig::default()),
        false,
    );
    let dio1 = wake_input::V4Input::new(
        Input::new(peripherals.GPIO14, InputConfig::default()),
        cfg!(feature = "host-uart-low-power"),
    );
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

    let modulation = match lora.create_modulation_params(
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
    let tx_params = match lora.create_tx_packet_params(16, false, true, false, &modulation) {
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
    let rx_params = match lora.create_rx_packet_params(16, false, 255, true, false, &modulation) {
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

    // Past every path that hands the halves to `serve_status_only`, so they can become the
    // host link and the radio settings become the state `radio-hand` drives.
    let mut host = host::SplitHost::new(usb_rx, usb_tx);
    let mut radio = RadioState {
        modulation,
        tx: tx_params,
        rx: rx_params,
        tx_power_dbm: i32::from(board::DEFAULT_TX_POWER_DBM),
        prepare_rx: true,
    };
    let face = Face {
        publish: ui::publish,
        publish_host: ui::publish_host,
    };

    let _ = host.write_all(online).await;
    let mut command_stream = CommandStream::new();
    let mut usb_command = [0_u8; MAX_COMMAND_LEN];
    // No persistence and no entropy on this board yet: gate N0 gave the T114 a store and
    // left this one without. It refuses rather than inventing either.
    let mut no_store = NoStore;

    // The proof enters Light-sleep from the radio task itself. This preserves the pending
    // receive future across sleep rather than asking the scheduler's idle hook to do so.
    #[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
    let mut proof_rtc = esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR);
    #[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
    let mut proof_sleep_enabled = false;

    // Hand the RTC to the idle hook. Only now is sleeping meaningful: the radio is about to be
    // armed for continuous receive, so a sleeping CPU still hears packets.
    #[cfg(all(feature = "host-uart-low-power", not(feature = "rf-sleep-proof")))]
    power::arm(esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR));

    loop {
        if radio.prepare_rx {
            // Configuring the radio is SPI traffic; sleeping through it would abandon a
            // half-finished transaction.
            let _awake = power::Awake::new();
            if lora
                .prepare_for_rx(RxMode::Continuous, &radio.modulation, &radio.rx)
                .await
                .is_err()
            {
                local_status.radio = radio_face::RadioState::Fault;
                local_status.fault = Some(radio_face::Fault {
                    code: 5,
                    message: radio_face::Text::from_truncated("RX SETUP"),
                });
                ui::publish(local_status, radio_face::LedSignal::Idle);
                let _ = host.write_all(b"radio rx setup failed\r\n").await;
                continue;
            }
            local_status.radio = radio_face::RadioState::Online;
            local_status.fault = None;
            ui::publish(local_status, radio_face::LedSignal::Idle);
            radio.prepare_rx = false;
        }

        let mut usb_packet = [0_u8; 64];
        let mut radio_frame = [0_u8; MAX_RADIO_FRAME];
        // The one point this loop is genuinely idle: both sides are merely waiting, the radio
        // is listening on its own, and nothing is half-done. Sleeping is allowed here and
        // nowhere else — but only at a frame boundary, since a wake eats the bytes that
        // triggered it and would truncate a command already in progress.
        #[cfg(not(feature = "rf-sleep-proof"))]
        let host_read = host.read(&mut usb_packet);
        #[cfg(feature = "rf-sleep-proof")]
        let host_read = core::future::pending::<Result<usize, radio_hand::link::LinkFault>>();
        #[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
        let radio_receive = async {
            if !proof_sleep_enabled {
                return lora.rx(&radio.rx, &mut radio_frame).await;
            }

            let mut receive = core::pin::pin!(lora.rx(&radio.rx, &mut radio_frame));
            poll_fn(|cx| match receive.as_mut().poll(cx) {
                Poll::Ready(outcome) => Poll::Ready(outcome),
                Poll::Pending if wake_input::radio_wake_armed() && !wake_input::radio_is_high() => {
                    let elapsed = power::sleep_once(&mut proof_rtc);
                    if elapsed >= 4_500_000 {
                        // The RTC safety timer has no Embassy future to wake. GPIO and rejected
                        // sleeps already have their own pending interrupt or radio waiter.
                        cx.waker().wake_by_ref();
                    }
                    Poll::Pending
                }
                Poll::Pending => {
                    // `lora.rx()` may first yield while its asynchronous SPI setup is still
                    // putting the SX1262 into receive. Re-poll until the DIO1 waiter confirms
                    // that both continuous receive and its CPU/wake interrupts are armed.
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
            .await
        };
        #[cfg(not(all(feature = "host-uart-low-power", feature = "rf-sleep-proof")))]
        let radio_receive = lora.rx(&radio.rx, &mut radio_frame);
        let waiting = select(host_read, radio_receive);
        let outcome = if command_stream.is_boundary() {
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
                local_status.rx_frames = local_status.rx_frames.saturating_add(1);
                local_status.last_rx = Some(radio_face::RxSummary {
                    frame_len: length as u16,
                    rssi_dbm: packet_status.rssi,
                    snr_tenths_db: packet_status.snr.saturating_mul(10),
                });
                local_status.last_wake = radio_face::WakeSource::Radio;
                ui::publish(local_status, radio_face::LedSignal::Activity);

                // The low-power board's UART host is intentionally absent from this bench.
                // A feature-gated RF challenge therefore returns the counters through the
                // already attached T114. Each matching receipt proves that this exact RF
                // frame resumed the CPU after at least the reported number of sleep entries.
                #[cfg(feature = "rf-sleep-proof")]
                if let Some(nonce) = sleep_proof_nonce(&radio_frame[..length]) {
                    let (sleep_entries, _) = power::counters();
                    let receipt = sleep_proof_receipt(
                        nonce,
                        sleep_entries,
                        wake_input::radio_wake_registrations(),
                        local_status.rx_frames,
                        power::last_sleep_us(),
                        proof_sleep_enabled,
                        proof_reset_reason,
                    );
                    // Give the T114 time to finish its TX acknowledgement and re-arm RX.
                    // This deliberately uses a blocking HAL delay rather than introducing
                    // an Embassy timer into the very clock behavior under examination.
                    BlockingDelay::new().delay_millis(250);
                    let sent = lora
                        .prepare_for_tx(
                            &radio.modulation,
                            &mut radio.tx,
                            radio.tx_power_dbm,
                            &receipt,
                        )
                        .await
                        .is_ok()
                        && lora.tx().await.is_ok();
                    radio.prepare_rx = true;
                    if sent {
                        local_status.tx_frames = local_status.tx_frames.saturating_add(1);
                        local_status.last_tx = radio_face::TxResult::Sent {
                            frame_len: receipt.len() as u16,
                        };
                    } else {
                        local_status.last_tx = radio_face::TxResult::Failed { code: 1 };
                    }
                    ui::publish(local_status, radio_face::LedSignal::Activity);
                    #[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
                    {
                        // The first matching challenge is the awake control. Every subsequent
                        // receive begins with a registered DIO1 waiter followed by Light-sleep.
                        proof_sleep_enabled = true;
                    }
                }

                let mut event = [0_u8; 7 + MAX_RADIO_FRAME];
                event[0] = EVENT_RX;
                event[1..3].copy_from_slice(&(length as u16).to_le_bytes());
                event[3..5].copy_from_slice(&packet_status.rssi.to_le_bytes());
                event[5..7].copy_from_slice(&packet_status.snr.to_le_bytes());
                event[7..7 + length].copy_from_slice(&radio_frame[..length]);
                let _ = host.write_all(&event[..7 + length]).await;
            }
            Either::Second(Err(_)) => {
                local_status.radio = radio_face::RadioState::Fault;
                local_status.fault = Some(radio_face::Fault {
                    code: 6,
                    message: radio_face::Text::from_truncated("RADIO RX"),
                });
                ui::publish(local_status, radio_face::LedSignal::Idle);
                let _ = host.write_all(b"radio rx failed\r\n").await;
                radio.prepare_rx = true;
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
                if command_stream.is_boundary() {
                    let skip = packet
                        .iter()
                        .position(|byte| *byte != WAKE_BYTE)
                        .unwrap_or(packet.len());
                    packet = &packet[skip..];
                    if packet.is_empty() {
                        continue;
                    }
                }
                let at_boundary = command_stream.is_boundary();
                if at_boundary && (packet == b"status\n" || packet == b"status\r\n") {
                    let _ = host.write_all(online).await;
                    continue;
                }
                if at_boundary && (packet == b"sync\n" || packet == b"sync\r\n") {
                    let _ = host.write_all(b"2b 24b4\r\n").await;
                    continue;
                }
                if at_boundary && (packet == b"ui\n" || packet == b"ui\r\n") {
                    let diagnostic = ui::diagnostic();
                    let mut reply = radio_face::Text::<80>::empty();
                    let _ = write!(
                        &mut reply,
                        "ui={}; display={}; screen={}; button={}; host={}\r\n",
                        diagnostic.state,
                        diagnostic.display,
                        diagnostic.screen,
                        diagnostic.button,
                        diagnostic.host,
                    );
                    let _ = host.write_all(reply.as_str().as_bytes()).await;
                    continue;
                }
                #[cfg(feature = "ui-bench")]
                if at_boundary && (packet == b"fault\n" || packet == b"fault\r\n") {
                    local_status.radio = radio_face::RadioState::Fault;
                    local_status.fault = Some(radio_face::Fault {
                        code: 0xfe,
                        message: radio_face::Text::from_truncated("BENCH FAULT"),
                    });
                    ui::publish(local_status, radio_face::LedSignal::Idle);
                    let _ = host.write_all(b"ui bench fault set\r\n").await;
                    continue;
                }
                #[cfg(feature = "ui-bench")]
                if at_boundary && (packet == b"clear\n" || packet == b"clear\r\n") {
                    local_status.radio = radio_face::RadioState::Online;
                    local_status.fault = None;
                    ui::publish(local_status, radio_face::LedSignal::Idle);
                    let _ = host.write_all(b"ui bench fault cleared\r\n").await;
                    continue;
                }
                // Sleep diagnostics, for the power receipt: how many times the idle hook
                // actually slept, and how many times it wanted to but the gate was closed.
                // A build that never sleeps answers with zeros rather than nothing, so the
                // bench can tell "not sleeping" from "wrong firmware".
                if at_boundary && (packet == b"sleep\n" || packet == b"sleep\r\n") {
                    let (entries, blocked) = power::counters();
                    let mut report = [0_u8; 9];
                    report[0] = EVENT_DIAGNOSTIC;
                    report[1..5].copy_from_slice(&entries.to_le_bytes());
                    report[5..9].copy_from_slice(&blocked.to_le_bytes());
                    let _ = host.write_all(&report).await;
                    continue;
                }

                // An executive per call rather than one for the whole loop. This board's
                // receive path is still bespoke — the low-power proof polls `lora.rx()` by
                // hand so it can enter Light-sleep from inside the future — so it keeps its
                // own hand on the radio, and adopts the seam only where the shared command
                // loop needs it. The T114 holds one for the whole of `main` and gets the full
                // boundary; this board follows when the sleep work is settled.
                // INTERIM: this board has no store, so its region is a build fact rather
                // than a persisted one — US915, matching the bench it lives on. The honest
                // fix is a settings partition (pressure point 5's per-board backend); until
                // then, shipping this image outside the US is wrong by construction.
                let mut exec = Executive::new(
                    &mut lora,
                    &mut radio,
                    &mut local_status,
                    &face,
                    &mut no_store,
                    Region::Us915,
                );
                let outcome = dispatch::on_host_bytes(
                    &mut host,
                    &mut exec,
                    &mut command_stream,
                    &mut usb_command,
                    &Sx126xDiagnostics,
                    packet,
                )
                .await;
                // This board's transports never report `Detached`: USB Serial/JTAG buffers
                // into a peripheral that does not fail a write when the host leaves, and a
                // bare UART has nothing on the other end to notice. So the session never
                // ends, which is this board's existing behaviour, now falling out of the
                // shared loop rather than being written into it.
                debug_assert_eq!(outcome.flow, radio_hand::link::Flow::Continue);
            }
        }
    }
}
