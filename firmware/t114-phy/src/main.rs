#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_nrf::config::HfclkSource;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::spim::{Config as SpimConfig, Frequency, Spim};
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::{bind_interrupts, peripherals, usb};
use embassy_time::{Delay, Duration, with_timeout};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config, UsbDevice};
use embedded_hal_bus::spi::ExclusiveDevice;
use lora_phy::LoRa;
use lora_phy::sx126x::{Config as Sx126xConfig, Sx126x, Sx1262, TcxoCtrlVoltage};
use radio_hand::channel::modem::ModemChannel;
use radio_hand::channel::node::NodeChannel;
use radio_hand::channel::rnode::RNodeChannel;
use radio_hand::channel::{Channel, ChannelInfo, Event, Personality};
use radio_hand::executive::{Executive, Face, Heartbeat, RadioState};
use radio_hand::link::{Flow, HostLink};
use radio_hand::settings::Channel as BootChannel;
use selvage::{MESHTASTIC_SYNC_WORD, PhyProfile};
use static_cell::StaticCell;

use crate::radio::{Sx126xDiagnostics, T114Interface, T114Spi};

mod board;
mod control_fixture;
mod crash;
mod heap;
mod host;
mod le3;
mod lxmf;
mod probes;
mod radio;
mod store;
mod ui;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    TWISPI0 => embassy_nrf::spim::InterruptHandler<peripherals::TWISPI0>;
});

type UsbDriver = Driver<'static, HardwareVbusDetect>;

const TX_POWER_DBM: i32 = board::DEFAULT_TX_POWER_DBM as i32;
const MAX_RADIO_FRAME: usize = 255;
const USB_PACKET: usize = 64;

/// Boot line naming the node and its heap cost. Shows only the public destination.
fn describe_node(node: Option<&retinue::node::Node<32, 8, 4>>, out: &mut [u8; 64]) -> usize {
    let mut text = radio_face::Text::<64>::empty();
    match node {
        Some(node) => {
            let dest = node.destination();
            let bytes = dest.as_slice();
            let _ = write!(
                &mut text,
                "node={:02x}{:02x}{:02x}{:02x} heap={}/{} highwater={}\r\n",
                bytes[0],
                bytes[1],
                bytes[2],
                bytes[3],
                heap::used(),
                heap::HEAP_SIZE,
                heap::high_water(),
            );
        }
        None => {
            let _ = write!(&mut text, "node=unavailable\r\n");
        }
    }
    let source = text.as_str().as_bytes();
    let len = source.len().min(out.len());
    out[..len].copy_from_slice(&source[..len]);
    len
}

fn publish_fault(status: &mut radio_face::LocalStatus, code: u8, message: &'static str) {
    status.radio = radio_face::RadioState::Fault;
    status.fault = Some(radio_face::Fault {
        code,
        message: radio_face::Text::from_truncated(message),
    });
    ui::publish(*status, radio_face::LedSignal::Idle);
}

fn publish_online(status: &mut radio_face::LocalStatus) {
    status.radio = radio_face::RadioState::Online;
    status.fault = None;
    ui::publish(*status, radio_face::LedSignal::Idle);
}

#[embassy_executor::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver>) {
    device.run().await;
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    control_fixture::verify();
    // First statement: everything after this may allocate.
    // SAFETY: called once, before any allocation.
    unsafe { heap::init() };

    // Crash residue first: reset reason, crash count, and whether to distrust the channel.
    let boot_crash = crash::on_boot();

    let mut nrf_config = embassy_nrf::config::Config::default();
    nrf_config.hfclk_source = HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(nrf_config);

    // Watchdog: 8 s without executor progress resets the chip. Panics and hard faults
    // reboot through the crash handler instead.
    let watchdog_config = {
        let mut config = embassy_nrf::wdt::Config::default();
        config.timeout_ticks = 8 * 32768;
        config
    };
    if let Ok((_wdt, [handle])) =
        embassy_nrf::wdt::Watchdog::try_new::<_, 1>(p.WDT, watchdog_config)
        && let Ok(task) = crash::watchdog_task(handle)
    {
        spawner.spawn(task);
    }
    if let Ok(task) = crash::clean_run_task() {
        spawner.spawn(task);
    }

    // Settings first: a first boot erases and writes a flash page, stalling the CPU for
    // tens of milliseconds, which must stay clear of live traffic. The store lives for the
    // whole run because the executive owns its flash and entropy.
    let mut store = store::SettingsStore::new(p.NVMC, p.RNG);
    let mut identity_line = [0_u8; 48];
    let (mut settings, identity_line_len) = match store.load_or_create() {
        Ok((settings, outcome)) => (Some(settings), store::describe(outcome, &mut identity_line)),
        Err(_) => {
            let message = b"identity=unavailable\r\n";
            identity_line[..message.len()].copy_from_slice(message);
            (None, message.len())
        }
    };

    // Byte 1 is the native-node choice shipped before durable announce leases. Arm the new
    // channel guard before reserving or initializing the radio; if that write fails, stay in
    // modem recovery, or a downgrade could resume the old random-blob emitter.
    let mut native_guard_fault = false;
    if let Some(current) = settings
        && current.channel == BootChannel::LegacyNode
    {
        let guarded = radio_hand::settings::Settings {
            channel: BootChannel::Node,
            ..current
        };
        match store.save(&guarded) {
            Ok(_) => settings = Some(guarded),
            Err(_) => native_guard_fault = true,
        }
    }

    let node = settings
        .map(|settings| {
            retinue::node::Node::<32, 8, 4>::new(
                retinue::identity::PrivateIdentity::from_secret_bytes(&settings.identity),
                retinue::destination::DestinationName::new("retinue", ["node"]).name_hash(),
            )
            // Only the native node routes; modem and RNode stay host-driven.
            .with_transport_config(retinue::node::TransportConfig::transit())
        })
        .map(|mut node| {
            // Link request deadline includes the first hop's airtime (Ruling 50).
            let allowance = radio_hand::phy::nominal_bits_ms(
                board::DEFAULT_SPREADING_FACTOR,
                board::DEFAULT_BANDWIDTH_HZ,
                board::DEFAULT_CODING_RATE_DENOMINATOR,
                retinue::node::FIRST_HOP_ALLOWANCE_BITS,
            )
            .unwrap_or(0);
            let _ = node.set_first_hop_airtime(radio_hand::channel::node::RADIO, allowance);
            node
        });
    let mut node_line = [0_u8; 64];
    let node_line_len = describe_node(node.as_ref(), &mut node_line);

    // One durable announce lease per boot, before radio init. It lives apart from the
    // identity: a damaged lease selects the recovery modem without replacing the identity.
    let native_node_requested = settings
        .map(|settings| settings.channel.requests_native_node())
        .unwrap_or(false)
        && node.is_some()
        && !boot_crash.fallback;
    let lease = (native_node_requested && !native_guard_fault)
        .then(|| store.reserve_announce_timebase())
        .transpose();
    let mut timebase_fault = native_node_requested && (native_guard_fault || lease.is_err());
    let lease = lease.ok().flatten();
    let native_channel = node.zip(lease).and_then(|(node, lease)| {
        NodeChannel::new(node, lease)
            .map_err(|_| timebase_fault = true)
            .ok()
    });

    let mut display_config = SpimConfig::default();
    display_config.frequency = Frequency::M8;
    let display_spi = Spim::new_txonly(p.TWISPI0, Irqs, p.P1_08, p.P1_09, display_config);
    let display_cs = Output::new(p.P0_11, Level::High, OutputDrive::HighDrive);
    let display_dc = Output::new(p.P0_12, Level::Low, OutputDrive::Standard);
    let display_reset = Output::new(p.P0_02, Level::High, OutputDrive::Standard);
    let display_power = Output::new(p.P0_03, Level::High, OutputDrive::Standard);
    let display_backlight = Output::new(p.P0_15, Level::High, OutputDrive::Standard);
    let status_led = Output::new(p.P1_03, Level::High, OutputDrive::Standard);
    let button_rev21 = Input::new(p.P1_11, Pull::Up);
    let button_variant = Input::new(p.P1_10, Pull::Up);
    let mut local_status = board::initial_status();
    spawner.spawn(ui::button_task(button_rev21, button_variant).unwrap());
    let screen_hardware = ui::screen_hardware(
        display_spi,
        display_cs,
        display_dc,
        display_reset,
        display_power,
        display_backlight,
        status_led,
    );
    spawner.spawn(ui::screen_task(screen_hardware, local_status).unwrap());

    let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
    let mut usb_config = Config::new(0x1915, 0x521f);
    usb_config.manufacturer = Some("Tulle");
    usb_config.product = Some("T114 direct PHY");
    usb_config.serial_number = Some("TULLE-T114-01");
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;

    static STATE: StaticCell<State> = StaticCell::new();
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 128]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();
    let mut builder = Builder::new(
        driver,
        usb_config,
        &mut CONFIG_DESC.init([0; 256])[..],
        &mut BOS_DESC.init([0; 256])[..],
        &mut MSOS_DESC.init([0; 128])[..],
        &mut CONTROL_BUF.init([0; 128])[..],
    );
    let class = CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64);
    let usb = builder.build();
    match usb_task(usb) {
        Ok(task) => spawner.spawn(task),
        Err(_) => panic!(),
    }

    let spi = T114Spi {
        sck: Output::new(p.P0_19, Level::Low, OutputDrive::Standard),
        mosi: Output::new(p.P0_22, Level::Low, OutputDrive::Standard),
        miso: Input::new(p.P0_23, Pull::None),
    };
    let cs = Output::new(p.P0_24, Level::High, OutputDrive::Standard);
    let spi = match ExclusiveDevice::new(spi, cs, Delay) {
        Ok(spi) => spi,
        Err(_) => panic!(),
    };

    let reset = Output::new(p.P0_25, Level::High, OutputDrive::Standard);
    let dio1 = Input::new(p.P0_20, Pull::None);
    let busy = Input::new(p.P0_17, Pull::None);
    let interface = T114Interface { reset, dio1, busy };
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
    let init = with_timeout(
        Duration::from_secs(3),
        LoRa::new_with_sync_word(radio, MESHTASTIC_SYNC_WORD, Delay),
    )
    .await;
    let mut lora = match init {
        Ok(Ok(lora)) => lora,
        Ok(Err(_)) => {
            publish_fault(&mut local_status, 1, "SX1262 INIT");
            host::serve_status_only(
                class,
                b"tulle/t114 phy online; sx1262 init failed\r\n".as_slice(),
            )
            .await
        }
        Err(_) => {
            publish_fault(&mut local_status, 1, "SX1262 TIMEOUT");
            host::serve_status_only(
                class,
                b"tulle/t114 phy online; sx1262 init timed out\r\n".as_slice(),
            )
            .await
        }
    };

    // Boot carrier from the persisted region. With no region the board still tunes to the
    // US default for receiving, but the executive refuses every transmit.
    let region = settings.map(|s| s.region).unwrap_or_default();
    let boot_frequency = region
        .profile()
        .map(|p| p.default_frequency_hz)
        .unwrap_or(board::DEFAULT_FREQUENCY_HZ);
    // Same board defaults as the link deadline's airtime allowance (Ruling 70).
    let params = match (
        radio_hand::phy::spreading_factor(board::DEFAULT_SPREADING_FACTOR),
        radio_hand::phy::bandwidth(board::DEFAULT_BANDWIDTH_HZ),
        radio_hand::phy::coding_rate(board::DEFAULT_CODING_RATE_DENOMINATOR),
    ) {
        (Some(sf), Some(bw), Some(cr)) => lora
            .create_modulation_params(sf, bw, cr, boot_frequency)
            .ok(),
        _ => None,
    };
    let modulation = match params {
        Some(params) => params,
        None => {
            publish_fault(&mut local_status, 2, "PHY PARAMS");
            host::serve_status_only(class, b"tulle/t114 phy modulation invalid\r\n".as_slice())
                .await
        }
    };
    let tx_params = match lora.create_tx_packet_params(16, false, true, false, &modulation) {
        Ok(params) => params,
        Err(_) => {
            publish_fault(&mut local_status, 3, "TX PARAMS");
            host::serve_status_only(
                class,
                b"tulle/t114 phy tx parameters invalid\r\n".as_slice(),
            )
            .await
        }
    };
    let rx_params = match lora.create_rx_packet_params(16, false, 255, true, false, &modulation) {
        Ok(params) => params,
        Err(_) => {
            publish_fault(&mut local_status, 4, "RX PARAMS");
            host::serve_status_only(
                class,
                b"tulle/t114 phy rx parameters invalid\r\n".as_slice(),
            )
            .await
        }
    };

    let mut online_line = radio_face::Text::<320>::empty();
    let _ = write!(
        &mut online_line,
        "tulle/t114 phy online; version={}; sx1262 online; spi=software; irq=poll; \
         sync=2b reg=24b4; region={} freq={} reset={} crash={} timebase={}{}; build={}; image=t114-native\r\n",
        env!("CARGO_PKG_VERSION"),
        region.name(),
        boot_frequency,
        boot_crash.reset,
        boot_crash.count,
        if timebase_fault {
            "fault"
        } else if native_node_requested {
            "active"
        } else {
            "inactive"
        },
        if boot_crash.fallback {
            " FALLBACK=modem"
        } else if timebase_fault {
            " FALLBACK=modem"
        } else {
            ""
        },
        option_env!("RETINUE_FIRMWARE_REVISION").unwrap_or("unidentified"),
    );
    publish_online(&mut local_status);
    if timebase_fault {
        // Modem stays up for repair; native-node announces are denied this boot.
        publish_fault(&mut local_status, 7, "TIMEBASE");
    }

    let mut host = host::UsbHost::new(class);
    let mut radio = RadioState {
        profile: PhyProfile::meshtastic_long_fast(boot_frequency),
        modulation,
        tx: tx_params,
        rx: rx_params,
        tx_power_dbm: TX_POWER_DBM,
        prepare_rx: true,
    };
    let face = Face {
        publish: ui::publish,
        publish_host: ui::publish_host,
    };
    // The executive owns `lora`, the flash and the RNG for the rest of `main`.
    let mut exec = Executive::new(
        &mut lora,
        &mut radio,
        &mut local_status,
        &face,
        &mut store,
        region,
    );
    // Per-boot token from the RNG, no flash writes. Entropy failure disables observation.
    let mut observation_boot = [0; 8];
    let observation_boot = if exec.random(&mut observation_boot).is_ok() {
        u64::from_be_bytes(observation_boot)
    } else {
        0
    };
    let mut observations =
        radio_hand::observation::owner::OwnerObservations::new(observation_boot).ok();
    if let Some(observations) = observations.as_mut() {
        exec.attach_observations(observations);
    }

    // Personality is fixed per boot; switching requires a reboot. The listener-executive
    // plan supersedes structural decision 4, but is not wired into this loop yet.
    // No readable identity, or three consecutive crash boots, selects the modem: it needs
    // only a radio and a host. The crash count clears after a clean minute.
    let mut channel = match (
        settings.map(|s| s.channel),
        native_channel,
        boot_crash.fallback,
    ) {
        (Some(channel), Some(node), false) if channel.requests_native_node() => {
            Personality::Node(node)
        }
        // RNode needs no board identity; the host holds one.
        (Some(BootChannel::Rnode), _, false) => Personality::Rnode(RNodeChannel::new()),
        _ => Personality::Modem(ModemChannel::new(Sx126xDiagnostics)),
    };

    // Outside the session loop so announce cadence survives attach and detach.
    let mut heartbeat = Heartbeat::new(channel.heartbeat());

    loop {
        radio_hand::channel::await_host_with_listening(
            &mut channel,
            &mut exec,
            &mut host,
            &mut heartbeat,
            true,
        )
        .await;
        exec.status_mut().host = radio_face::HostState::Attached;
        exec.publish(radio_face::LedSignal::Idle);
        // Plain-text greeting, unless the channel speaks a binary protocol from byte one.
        let greeted = !channel.banner()
            || (host
                .write_all(online_line.as_str().as_bytes())
                .await
                .is_ok()
                && host
                    .write_all(&identity_line[..identity_line_len])
                    .await
                    .is_ok()
                && host.write_all(&node_line[..node_line_len]).await.is_ok());
        if !greeted || channel.start(&mut exec, &mut host).await == Flow::Detach {
            host.require_detach();
            exec.status_mut().host = radio_face::HostState::Detached;
            exec.publish(radio_face::LedSignal::Idle);
            continue;
        }

        loop {
            match exec.ensure_rx().await {
                Ok(true) => publish_online(exec.status_mut()),
                Ok(false) => {}
                Err(_) => {
                    publish_fault(exec.status_mut(), 5, "RX SETUP");
                    if host.write_all(b"radio rx setup failed\r\n").await.is_err() {
                        break;
                    }
                    continue;
                }
            }

            let mut usb_packet = [0_u8; USB_PACKET];
            let mut radio_frame = [0_u8; MAX_RADIO_FRAME];
            // Bound, not matched in place, so the futures' borrows end before an arm takes
            // the executive. Only `wait_rx_irq` is raced: cancelling a whole receive after
            // its interrupt fired would consume the IRQ and lose the frame.
            let woken = select3(
                host.read(&mut usb_packet),
                exec.wait_rx_irq(),
                heartbeat.next(),
            )
            .await;
            match woken {
                Either3::Second(Ok(())) => {
                    // Deliberately not raced: the frame is in the radio until it is read.
                    let collected = exec.collect(&mut radio_frame).await;
                    let received = match collected {
                        // CRC failure: counted in `rx_damaged`; wait for the next frame.
                        Ok(None) => continue,
                        Ok(Some(received)) => received,
                        Err(_) => {
                            publish_fault(exec.status_mut(), 6, "RADIO RX");
                            if host.write_all(b"radio rx failed\r\n").await.is_err() {
                                break;
                            }
                            continue;
                        }
                    };
                    let flow = channel
                        .serve(
                            &mut exec,
                            &mut host,
                            Event::RadioFrame {
                                frame: &radio_frame[..received.len],
                                rssi: received.rssi,
                                snr: received.snr,
                            },
                        )
                        .await;
                    if flow == Flow::Detach {
                        break;
                    }
                }
                Either3::Second(Err(_)) => {
                    publish_fault(exec.status_mut(), 6, "RADIO RX");
                    if host.write_all(b"radio rx failed\r\n").await.is_err() {
                        break;
                    }
                }
                Either3::Third(()) => {
                    if channel.serve(&mut exec, &mut host, Event::Beat).await == Flow::Detach {
                        break;
                    }
                }
                Either3::First(Err(_)) => break,
                Either3::First(Ok(length)) => {
                    let status = exec.status_mut();
                    status.host = radio_face::HostState::Attached;
                    status.last_wake = radio_face::WakeSource::Host;
                    exec.publish(radio_face::LedSignal::Idle);
                    let packet = &usb_packet[..length];
                    let at_boundary = channel.at_boundary();
                    match probes::handle(
                        packet,
                        at_boundary,
                        &online_line,
                        settings,
                        &Sx126xDiagnostics,
                        &mut exec,
                        &mut host,
                    )
                    .await
                    {
                        probes::Outcome::NotAProbe => {}
                        probes::Outcome::Served => continue,
                        probes::Outcome::HostGone => break,
                    }
                    let flow = channel
                        .serve(&mut exec, &mut host, Event::HostBytes(packet))
                        .await;
                    if flow == Flow::Detach {
                        break;
                    }
                }
            }
        }
        channel.stop(&mut exec, &mut host).await;
        // DTR drops slightly after the failed I/O that ended the session. Require that edge
        // before the next attach, or a latched DTR can hang the banner write on a vanished
        // host. The unattached loop keeps servicing the radio meanwhile; never block here.
        host.require_detach();
        exec.status_mut().host = radio_face::HostState::Detached;
        exec.publish(radio_face::LedSignal::Idle);
    }
}
