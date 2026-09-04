#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(all(feature = "host-usb", feature = "host-uart-low-power"))]
compile_error!("select exactly one V4 host transport: host-usb or host-uart-low-power");
#[cfg(not(any(feature = "host-usb", feature = "host-uart-low-power")))]
compile_error!("select exactly one V4 host transport: host-usb or host-uart-low-power");

#[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
use core::future::{Future, poll_fn};
#[cfg(all(feature = "host-uart-low-power", feature = "rf-sleep-proof"))]
use core::task::Poll;

use embassy_executor::Spawner;
use embassy_futures::select::Either3;
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use embassy_futures::select::select3;
#[cfg(feature = "host-uart-low-power")]
use embassy_futures::select::{Either, select};
use embassy_time::Delay;
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use embassy_time::{Instant, Timer};
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
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use lora_modulation::{Bandwidth, CodingRate, SpreadingFactor};
use lora_phy::LoRa;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{Config as Sx126xConfig, Sx126x, Sx1262, TcxoCtrlVoltage};
use radio_hand::dispatch;
use radio_hand::executive::{ChipDiagnostics, Face, RadioState};
use radio_hand::link::HostLink;
use selvage::{
    CommandStream, EVENT_DIAGNOSTIC, EVENT_RX, MAX_COMMAND_LEN, MESHTASTIC_SYNC_WORD, WAKE_BYTE,
};

mod board;
mod channels;
mod commissioning_store;
mod control_boot;
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
mod control_carrier;
mod control_fixture;
mod control_store;
mod gnss;
mod host;
#[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
mod physical_presence;
mod power;
mod radio_owner;
#[cfg(feature = "rf-sleep-proof")]
mod sleep_proof;
mod store;
mod ui;
mod wake_input;
mod wake_lease;

esp_bootloader_esp_idf::esp_app_desc!();

/// Host UART line rate for the low-power personality. Matches the host default in
/// `tulle::direct_phy_serial`.
#[cfg(feature = "host-uart-low-power")]
const HOST_UART_BAUD: u32 = 115_200;

const MAX_RADIO_FRAME: usize = 255;

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
    control_fixture::verify();
    let peripherals = esp_hal::init(Config::default());
    #[cfg(feature = "rf-sleep-proof")]
    let proof_reset_reason = esp_hal::system::reset_reason()
        .map(|reason| reason as u32)
        .unwrap_or_default();

    // Settings before anything else, and before the radio: a first boot erases and writes
    // a flash sector, so it belongs here rather than anywhere near live traffic. The store
    // holds the ADC entropy source for the life of the board — see `store.rs` for why the
    // obvious `Rng` would have minted a predictable identity key.
    let mut store = store::SettingsStore::new(peripherals.FLASH, peripherals.RNG, peripherals.ADC1);
    let mut identity_line = [0_u8; 48];
    let (settings, identity_line_len) = match store.load_or_create() {
        Ok((settings, outcome)) => (Some(settings), store::describe(outcome, &mut identity_line)),
        Err(_) => {
            let message = b"identity=unavailable\r\n";
            identity_line[..message.len()].copy_from_slice(message);
            (None, message.len())
        }
    };

    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);

    // The USB personality keeps the stock idle loop: its host link cannot survive Light-sleep,
    // so there is nothing to gain and a re-enumeration failure to lose.
    #[cfg(any(
        all(feature = "host-usb", not(feature = "host-uart-low-power")),
        feature = "rf-sleep-proof"
    ))]
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // The low-power personality installs a gated idle hook instead. It only sleeps once
    // `power::arm` hands it the RTC, which happens after the radio is receiving.
    #[cfg(all(feature = "host-uart-low-power", not(feature = "rf-sleep-proof")))]
    esp_rtos::start_with_idle_hook(timg0.timer0, sw_int.software_interrupt0, power::idle);

    // The host link. Both personalities implement `embedded_io_async::{Read, Write}`, so
    // everything below is written once and built twice.
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
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

    // GNSS socket (PD3). Heltec's V4 factory sketch gives UART1 as RX GPIO39, TX GPIO38.
    // The L76K is read-only here, so own only module-TX / ESP-RX GPIO39 and do not drive
    // module-RX GPIO38. GPIO34 enables the separate GNSS rail active low; Vext GPIO36
    // above supplies the OLED/external rail, not this socket.
    let gnss_rx = esp_hal::uart::UartRx::new(
        peripherals.UART1,
        esp_hal::uart::Config::default().with_baudrate(gnss::BAUD),
    )
    .unwrap()
    .with_rx(peripherals.GPIO39)
    .into_async();
    let gnss_pins = gnss::ControlPins {
        enable: Output::new(peripherals.GPIO34, Level::Low, OutputConfig::default()),
        reset: Output::new(peripherals.GPIO42, Level::High, OutputConfig::default()),
        standby: Output::new(peripherals.GPIO40, Level::High, OutputConfig::default()),
    };
    spawner.spawn(gnss::gnss_task(gnss_rx, gnss_pins).unwrap());
    let mut local_status = board::initial_status();
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

    let online = concat!(
        "tulle/heltec-v4 phy online; version=",
        env!("CARGO_PKG_VERSION"),
        "; sx1262 online; sync=2b reg=24b4; longfast=906875000\r\n",
    )
    .as_bytes();
    local_status.radio = radio_face::RadioState::Online;
    local_status.fault = None;
    ui::publish(local_status, radio_face::LedSignal::Idle);

    // Past every path that hands the halves to `serve_status_only`, so they can become the
    // host link and the radio settings become the state `radio-hand` drives.
    let radio = RadioState {
        profile: selvage::PhyProfile::meshtastic_long_fast(board::DEFAULT_FREQUENCY_HZ),
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
    let mut owner =
        radio_owner::V4RadioOwner::new(lora, radio, local_status, face, store, settings);

    // WN1 durable recovery is a boot-only action: before a host exists, before RNode gets
    // exclusive custody, and before any receive or sleep machinery starts. A missing settings
    // record retains the existing recovery modem posture. Any WN1 error is a status-only boot;
    // it never proceeds into modem or RNode service with uncertain durable state.
    // Only the USB image's signed carrier borrows the resident runtime mutably.
    #[cfg_attr(feature = "host-uart-low-power", allow(unused_mut))]
    let (button, mut control_ready) = if let Some(settings) = settings {
        // Safety: board startup creates this exactly once after a real ESP reset, before host
        // construction, RNode selection, RX arming, `power::arm`, or any radio service. The
        // consumed token confines `ControlRuntime::new_after_hardware_reset` to boot recovery.
        let reset = unsafe { control_boot::after_hardware_reset() };
        let (button, boot) = control_boot::boot_pre_radio_owner(
            reset,
            &mut owner,
            &settings.identity,
            button,
            &mut usb_rx,
            &mut usb_tx,
        )
        .await;
        match boot {
            Ok(control_boot::ControlBootOutcome::ControlReady(ready)) => (button, Some(ready)),
            Ok(control_boot::ControlBootOutcome::BlankUncommissioned) => (button, None),
            Ok(control_boot::ControlBootOutcome::FirstWritePending) => {
                owner.radio_fault(8, "CONTROL PENDING");
                serve_status_only(
                    usb_rx,
                    usb_tx,
                    b"tulle/heltec-v4 phy control first-write pending\r\n",
                )
                .await
            }
            Err(control_boot::ControlBootError::FirstWriteStore(error)) => {
                let _ = error;
                owner.radio_fault(9, "FIRSTWRITE STORE");
                serve_status_only(
                    usb_rx,
                    usb_tx,
                    b"tulle/heltec-v4 phy first-write storage failed\r\n",
                )
                .await
            }
            Err(control_boot::ControlBootError::EntropyUnavailable) => {
                owner.radio_fault(10, "CLAIM ENTROPY");
                serve_status_only(
                    usb_rx,
                    usb_tx,
                    b"tulle/heltec-v4 phy claim entropy unavailable\r\n",
                )
                .await
            }
            Err(control_boot::ControlBootError::OwnerUnavailable) => {
                owner.radio_fault(11, "CONTROL OWNER");
                serve_status_only(
                    usb_rx,
                    usb_tx,
                    b"tulle/heltec-v4 phy control boot owner unavailable\r\n",
                )
                .await
            }
            Err(control_boot::ControlBootError::Runtime(error)) => {
                let _ = error;
                owner.radio_fault(12, "CONTROL BOOT");
                serve_status_only(
                    usb_rx,
                    usb_tx,
                    b"tulle/heltec-v4 phy control runtime boot failed\r\n",
                )
                .await
            }
        }
    } else {
        (button, None)
    };

    spawner.spawn(ui::button_task(button).unwrap());

    let mut host = host::SplitHost::new(usb_rx, usb_tx);

    // The channel selector. The RNode loop never returns: switching back is a persisted
    // byte and a reboot, and no banner is written because a host opening that port expects
    // KISS frames from the first byte. The sleep-proof bench build keeps the modem
    // unconditionally; it is a bench, not a shipping personality.
    #[cfg(not(feature = "rf-sleep-proof"))]
    if settings.map(|s| s.channel) == Some(radio_hand::settings::Channel::Rnode) {
        channels::serve_rnode(owner, online, &identity_line[..identity_line_len], host).await
    }

    let _ = host.write_all(online).await;
    let _ = host.write_all(&identity_line[..identity_line_len]).await;
    let mut command_stream = CommandStream::new();
    let mut usb_command = [0_u8; MAX_COMMAND_LEN];
    let mut control_stream = channels::ControlFrameStream::new();
    #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
    let mut control_carrier = control_carrier::ControlCarrier::new();

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
        {
            // Configuring the radio is SPI traffic; sleeping through it would abandon a
            // half-finished transaction.
            let _awake = power::Awake::new();
            match owner.ensure_rx().await {
                Ok(true) => owner.radio_online(),
                Ok(false) => {}
                Err(radio_owner::RxSetupFault::Prepare) => {
                    owner.radio_fault(5, "RX SETUP");
                    let _ = host.write_all(b"radio rx setup failed\r\n").await;
                    continue;
                }
                Err(radio_owner::RxSetupFault::Arm) => {
                    let _ = host.write_all(b"radio rx arm failed\n").await;
                    continue;
                }
            }
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
                return owner.wait_rx_irq().await;
            }

            // RX was prepared and armed before this wait. Keep the proof's sleep polling around
            // the cancellation-safe IRQ waiter, then let the common match collect the frame
            // without racing it.
            let mut receive = core::pin::pin!(owner.wait_rx_irq());
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
                    // The IRQ waiter may still be registering the DIO1 interrupt and wake bit.
                    // Re-poll until that registration is visible before sleeping.
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
            .await
        };
        // Only the interrupt wait is raced. Racing a whole receive cancels it wherever it
        // stands, and once its interrupt has fired that abandons a frame midway out of the
        // chip: interrupt consumed, bytes left for the next packet to overwrite, nothing
        // reported. This half holds no transaction and leaves the radio listening.
        #[cfg(not(all(feature = "host-uart-low-power", feature = "rf-sleep-proof")))]
        let radio_receive = owner.wait_rx_irq();
        // The USB image also waits on the armed candidate's deadline, so an unconfirmed
        // provisional change rolls back on time without a host frame to prompt it.
        #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
        let expiry = async {
            match control_ready
                .as_ref()
                .and_then(|ready| ready.runtime.provisional_deadline_ms())
            {
                Some(deadline_ms) => Timer::at(Instant::from_millis(deadline_ms)).await,
                None => core::future::pending::<()>().await,
            }
        };
        #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
        let waiting = async {
            match select3(host_read, radio_receive, expiry).await {
                Either3::First(host) => Either3::First(host),
                Either3::Second(radio) => Either3::Second(radio),
                Either3::Third(()) => Either3::Third(()),
            }
        };
        #[cfg(not(all(feature = "host-usb", not(feature = "host-uart-low-power"))))]
        let waiting = async {
            match select(host_read, radio_receive).await {
                Either::First(host) => Either3::First(host),
                Either::Second(radio) => Either3::Second(radio),
            }
        };
        let outcome = if command_stream.is_boundary() {
            waiting.await
        } else {
            let _awake = power::Awake::new();
            waiting.await
        };
        // Everything past here touches SPI, the radio, or the host link.
        let _awake = power::Awake::new();
        match outcome {
            Either3::Third(()) => {
                #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
                if let Some(ready) = control_ready.as_mut()
                    && !control_carrier::expire_provisional(&mut owner, ready).await
                {
                    // Not at a quiet boundary yet: give the radio a moment and look again.
                    Timer::after_millis(200).await;
                }
            }
            Either3::Second(Ok(())) => {
                // Deliberately not raced: the frame is in the radio until it is read out.
                let Some(frame) = (match owner.collect(&mut radio_frame).await {
                    Ok(frame) => frame,
                    Err(_) => {
                        // Said out loud, matching the T114. A radio that stops receiving is
                        // the whole failure on a board whose only job is receiving.
                        owner.radio_fault(6, "RADIO RX");
                        if host.write_all(b"radio rx failed\r\n").await.is_err() {
                            break;
                        }
                        continue;
                    }
                }) else {
                    continue;
                };
                let length = frame.len;
                owner.note_radio_frame(&frame);

                // The low-power board's UART host is intentionally absent from this bench.
                // A feature-gated RF challenge therefore returns the counters through the
                // already attached T114. Each matching receipt proves that this exact RF
                // frame resumed the CPU after at least the reported number of sleep entries.
                #[cfg(feature = "rf-sleep-proof")]
                if let Some(nonce) = sleep_proof::nonce(&radio_frame[..length]) {
                    let (sleep_entries, _) = power::counters();
                    let status = owner.status();
                    let receipt = sleep_proof::receipt(
                        nonce,
                        sleep_entries,
                        wake_input::radio_wake_registrations(),
                        status.rx_frames,
                        power::last_sleep_us(),
                        proof_sleep_enabled,
                        proof_reset_reason,
                    );
                    // Give the T114 time to finish its TX acknowledgement and re-arm RX.
                    // This deliberately uses a blocking HAL delay rather than introducing
                    // an Embassy timer into the very clock behavior under examination.
                    BlockingDelay::new().delay_millis(250);
                    let sent = owner.proof_transmit(&receipt).await;
                    owner.note_proof_tx(receipt.len(), sent);
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
                event[3..5].copy_from_slice(&frame.rssi.to_le_bytes());
                event[5..7].copy_from_slice(&frame.snr.to_le_bytes());
                event[7..7 + length].copy_from_slice(&radio_frame[..length]);
                let _ = host.write_all(&event[..7 + length]).await;
            }
            // `wait_for_irq()` reports waiter/interrupt errors here. CRC and header outcomes
            // belong to the unraced `rx_collect()` above, where the frame is actually read.
            Either3::Second(Err(_)) => {
                owner.radio_fault(6, "RADIO RX");
                let _ = host.write_all(b"radio rx failed\r\n").await;
            }
            Either3::First(Err(_)) => {}
            Either3::First(Ok(0)) => {}
            Either3::First(Ok(length)) => {
                owner.note_host_activity();
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
                // A read which contains a diagnostic delimiter (or completes a
                // prior fragmented diagnostic frame) is demultiplexed one byte
                // at a time. Every non-diagnostic byte reaches the ordinary
                // direct-PHY parser immediately, so bytes before and after a
                // KISS request in one USB read are never discarded and a FEND
                // inside an ordinary command remains ordinary payload.
                if control_stream.in_frame() || packet.contains(&selvage::kiss::FEND) {
                    for &byte in packet {
                        let demux = control_stream.demux_byte(command_stream.is_boundary(), byte);
                        match demux {
                            channels::ControlDemux::Ordinary => {
                                let mut exec = owner.executive();
                                let outcome = dispatch::on_host_bytes(
                                    &mut host,
                                    &mut exec,
                                    &mut command_stream,
                                    &mut usb_command,
                                    &Sx126xDiagnostics,
                                    &[byte],
                                )
                                .await;
                                debug_assert_eq!(outcome.flow, radio_hand::link::Flow::Continue);
                            }
                            channels::ControlDemux::Consumed => {}
                            channels::ControlDemux::StatusRequest(request) => {
                                if let Some(ready) = control_ready.as_ref() {
                                    channels::send_control_status(
                                        ready.snapshot.with_query_nonce(request.nonce()),
                                        &mut host,
                                    )
                                    .await;
                                }
                            }
                            #[cfg(all(feature = "host-usb", not(feature = "host-uart-low-power")))]
                            channels::ControlDemux::Command => {
                                if let Some(ready) = control_ready.as_mut() {
                                    control_carrier
                                        .serve(control_stream.frame(), &mut owner, ready, &mut host)
                                        .await;
                                }
                            }
                        }
                    }
                    continue;
                }
                // The probes both channels share: status, sync, ui, region, and the channel
                // selector itself. Only at a boundary, so a probe inside a framed command is
                // carried rather than obeyed.
                if at_boundary
                    && matches!(
                        owner
                            .probe(
                                packet,
                                online,
                                &identity_line[..identity_line_len],
                                &mut host
                            )
                            .await,
                        channels::Outcome::Served
                    )
                {
                    continue;
                }
                #[cfg(feature = "ui-bench")]
                if at_boundary && (packet == b"fault\n" || packet == b"fault\r\n") {
                    owner.radio_fault(0xfe, "BENCH FAULT");
                    let _ = host.write_all(b"ui bench fault set\r\n").await;
                    continue;
                }
                #[cfg(feature = "ui-bench")]
                if at_boundary && (packet == b"clear\n" || packet == b"clear\r\n") {
                    owner.radio_online();
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

                // An executive per command inside the long-lived V4 owner. This board's
                // receive path is still bespoke — the low-power proof polls the IRQ waiter by
                // hand around Light-sleep and leaves frame collection to the common, unraced
                // path — but it can no longer split radio or store custody from the command
                // path.
                let mut exec = owner.executive();
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
