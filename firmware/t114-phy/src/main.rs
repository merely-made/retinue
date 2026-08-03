#![no_std]
#![no_main]

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
use lora_modulation::{Bandwidth, CodingRate, SpreadingFactor};
use lora_phy::LoRa;
use lora_phy::sx126x::{Config as Sx126xConfig, Sx126x, Sx1262, TcxoCtrlVoltage};
use radio_hand::channel::modem::ModemChannel;
use radio_hand::channel::node::NodeChannel;
use radio_hand::channel::{Channel, ChannelInfo, Event, Personality};
use radio_hand::executive::{Executive, Face, Heartbeat, RadioState};
use radio_hand::link::{Flow, HostLink};
use radio_hand::region::Region;
use radio_hand::settings::Channel as BootChannel;
use selvage::MESHTASTIC_SYNC_WORD;
use static_cell::StaticCell;

use crate::radio::{Sx126xDiagnostics, T114Interface, T114Spi};

mod board;
mod crash;
mod heap;
mod host;
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

const TX_POWER_DBM: i32 = 17;
const MAX_RADIO_FRAME: usize = 255;
const USB_PACKET: usize = 64;

/// A boot line naming the node and what it costs, so the heap figure is a receipt rather
/// than an assertion. The destination is public by construction; no key material is shown.
fn describe_node(node: Option<&retinue::node::Node<32, 8, 4>>, out: &mut [u8; 64]) -> usize {
    let mut text = radio_face::Text::<64>::empty();
    match node {
        Some(node) => {
            let dest = node.destination();
            let bytes = dest.as_slice();
            let _ = write!(
                &mut text,
                "node={:02x}{:02x}{:02x}{:02x} heap={}/{}\r\n",
                bytes[0],
                bytes[1],
                bytes[2],
                bytes[3],
                heap::used(),
                heap::HEAP_SIZE,
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

/// What a `channel` line asked for.
enum ChannelProbe {
    /// `channel` — say which personality boots.
    Report,
    /// `channel modem` or `channel node` — persist a choice and reboot into it.
    Set(BootChannel),
}

/// Read a host line as a channel probe, tolerating either line ending.
fn channel_probe(packet: &[u8]) -> Option<ChannelProbe> {
    let line = packet
        .strip_suffix(b"\r\n")
        .or_else(|| packet.strip_suffix(b"\n"))?;
    match line {
        b"channel" => Some(ChannelProbe::Report),
        b"channel modem" => Some(ChannelProbe::Set(BootChannel::Modem)),
        b"channel node" => Some(ChannelProbe::Set(BootChannel::Node)),
        _ => None,
    }
}

/// What a `region` line asked for.
enum RegionProbe {
    /// `region` — say which compliance profile the board operates under.
    Report,
    /// `region us915` and friends — persist a choice and reboot into it.
    Set(Region),
}

/// Read a host line as a region probe. Names match the table case-insensitively, so the
/// probe vocabulary grows when the table does, not when this function does.
fn region_probe(packet: &[u8]) -> Option<RegionProbe> {
    let line = packet
        .strip_suffix(b"\r\n")
        .or_else(|| packet.strip_suffix(b"\n"))?;
    if line == b"region" {
        return Some(RegionProbe::Report);
    }
    let name = line.strip_prefix(b"region ")?;
    Region::choices()
        .find(|region| region.name().as_bytes().eq_ignore_ascii_case(name))
        .map(RegionProbe::Set)
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
    // First statement: everything after this may allocate.
    // SAFETY: called once, before any allocation.
    unsafe { heap::init() };

    // The crash residue, before anything else can crash: reset reason, consecutive-crash
    // count, and whether this boot should distrust the persisted channel.
    let boot_crash = crash::on_boot();

    let mut nrf_config = embassy_nrf::config::Config::default();
    nrf_config.hfclk_source = HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(nrf_config);

    // The watchdog, armed as early as possible: 8 s of silence from the executor resets
    // the chip. Petting is a task, so what it proves is that the executor still breathes —
    // panics and hard faults reboot themselves through the crash handler without it.
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

    // Resolve the board's settings before anything else starts. A first boot
    // erases and writes a flash page, which stalls the CPU for tens of
    // milliseconds, so it belongs here rather than anywhere near live traffic.
    // The identity stays on the board; the channel says what to boot into.
    // The store stays alive for the whole run rather than being read and dropped: the
    // executive owns the board's flash and entropy, and both live here.
    let mut store = store::SettingsStore::new(p.NVMC, p.RNG);
    let mut identity_line = [0_u8; 48];
    let (settings, identity_line_len) = match store.load_or_create() {
        Ok((settings, outcome)) => (Some(settings), store::describe(outcome, &mut identity_line)),
        Err(_) => {
            let message = b"identity=unavailable\r\n";
            identity_line[..message.len()].copy_from_slice(message);
            (None, message.len())
        }
    };

    // The node this board answers as, built from the persisted identity.
    let node = settings.map(|settings| {
        retinue::node::Node::<32, 8, 4>::new(
            retinue::identity::PrivateIdentity::from_secret_bytes(&settings.identity),
            retinue::destination::DestinationName::new("retinue", ["node"]).name_hash(),
        )
    });
    let mut node_line = [0_u8; 64];
    let node_line_len = describe_node(node.as_ref(), &mut node_line);

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
    // Moved whole into either `serve_status_only` or the host link below, so it is never
    // mutated here.
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

    // The boot carrier comes from the persisted region: each region entry names the
    // trunk's default frequency inside its band. A board with no region still tunes (to the
    // US default) so RECEIVING works — receiving is unregulated — but the executive refuses
    // every transmit until a region is chosen.
    let region = settings.map(|s| s.region).unwrap_or_default();
    let boot_frequency = region
        .profile()
        .map(|p| p.default_frequency_hz)
        .unwrap_or(906_875_000);
    let modulation = match lora.create_modulation_params(
        SpreadingFactor::_11,
        Bandwidth::_250KHz,
        CodingRate::_4_5,
        boot_frequency,
    ) {
        Ok(params) => params,
        Err(_) => {
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

    // The banner names the region, the carrier, the reset reason, and any crash residue —
    // the facts a bench or a user needs before trusting the boot.
    let mut online_line = radio_face::Text::<192>::empty();
    let _ = write!(
        &mut online_line,
        "tulle/t114 phy online; sx1262 online; spi=software; irq=poll; sync=2b reg=24b4; \
         region={} freq={} reset={} crash={}{}\r\n",
        region.name(),
        boot_frequency,
        boot_crash.reset,
        boot_crash.count,
        if boot_crash.fallback {
            " FALLBACK=modem"
        } else {
            ""
        },
    );
    let online = online_line.as_str().as_bytes();
    publish_online(&mut local_status);

    // Past every path that hands `class` to `serve_status_only`, so the CDC endpoint can
    // become the host link and the radio can pass to the executive that owns it.
    let mut host = host::UsbHost::new(class);
    let mut radio = RadioState {
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
    // Held for the rest of `main`, which is what makes the boundary real on this board:
    // nothing below can reach `lora`, the flash, or the RNG again, because the executive
    // has all three.
    let mut exec = Executive::new(
        &mut lora,
        &mut radio,
        &mut local_status,
        &face,
        &mut store,
        region,
    );

    // The personality this board answers as, chosen from the persisted settings and fixed
    // for the life of the boot: switching is by reboot, per structural decision 4, so
    // nothing below ever has to hand the radio over.
    //
    // A board with no readable identity gets the modem regardless of what the settings ask
    // for. That is the recovery posture rather than a fallback of convenience: the modem
    // needs nothing but a radio and a host, so it is the one personality that cannot be
    // denied by a bad store.
    // A crash loop distrusts the persisted personality: three consecutive crash boots and
    // the board takes the channel that needs nothing, and says so on the banner. The count
    // clears after a clean minute, so the fallback is a refuge, not a trap.
    let mut channel = match (settings.map(|s| s.channel), node, boot_crash.fallback) {
        (Some(BootChannel::Node), Some(node), false) => Personality::Node(NodeChannel::new(node)),
        _ => Personality::Modem(ModemChannel::new(Sx126xDiagnostics)),
    };

    // Outside the session loop on purpose. A channel that runs without a host keeps its
    // clock across every attach and detach, so its announce cadence is the board's own and
    // not a function of when somebody plugged in a cable.
    let mut heartbeat = Heartbeat::new(channel.heartbeat());

    loop {
        radio_hand::channel::await_host(&mut channel, &mut exec, &mut host, &mut heartbeat).await;
        exec.status_mut().host = radio_face::HostState::Attached;
        exec.publish(radio_face::LedSignal::Idle);
        if host.write_all(online).await.is_err()
            || host
                .write_all(&identity_line[..identity_line_len])
                .await
                .is_err()
            || host.write_all(&node_line[..node_line_len]).await.is_err()
            || channel.start(&mut exec, &mut host).await == Flow::Detach
        {
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
            // Bound rather than matched in place, so the borrows the three futures hold end
            // here and an arm is free to take the executive again.
            let woken = select3(
                host.read(&mut usb_packet),
                exec.receive(&mut radio_frame),
                heartbeat.next(),
            )
            .await;
            match woken {
                Either3::Second(Ok(received)) => {
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
                        online,
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
        exec.status_mut().host = radio_face::HostState::Detached;
        exec.publish(radio_face::LedSignal::Idle);
    }
}
