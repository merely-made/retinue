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
use embassy_time::{Delay, Duration, Timer, with_timeout};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config, UsbDevice};
use embedded_hal_bus::spi::ExclusiveDevice;
use lora_modulation::{Bandwidth, CodingRate, SpreadingFactor};
use lora_phy::LoRa;
use lora_phy::sx126x::{Config as Sx126xConfig, Sx126x, Sx1262, TcxoCtrlVoltage};
use panic_halt as _;
use radio_hand::channel::modem::ModemChannel;
use radio_hand::channel::node::NodeChannel;
use radio_hand::channel::{Channel, ChannelInfo, Event, Personality};
use radio_hand::executive::{Executive, Face, Heartbeat, RadioState};
use radio_hand::link::{Flow, HostLink};
use radio_hand::region::Region;
use radio_hand::settings::{Channel as BootChannel, Settings};
use selvage::{MESHTASTIC_SYNC_WORD, sx126x_sync_word};
use static_cell::StaticCell;

use crate::radio::{Sx126xDiagnostics, T114Interface, T114Spi};

mod board;
mod heap;
mod host;
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

    let mut nrf_config = embassy_nrf::config::Config::default();
    nrf_config.hfclk_source = HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(nrf_config);

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

    // The banner names the region and carrier honestly instead of hardcoding a US channel.
    let mut online_line = radio_face::Text::<128>::empty();
    let _ = write!(
        &mut online_line,
        "tulle/t114 phy online; sx1262 online; spi=software; irq=poll; sync=2b reg=24b4; \
         region={} freq={}\r\n",
        region.name(),
        boot_frequency,
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
    let mut channel = match (settings.map(|s| s.channel), node) {
        (Some(BootChannel::Node), Some(node)) => Personality::Node(NodeChannel::new(node)),
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
                    if at_boundary && (packet == b"bootloader\n" || packet == b"bootloader\r\n") {
                        let _ = host.write_all(b"entering serial bootloader\r\n").await;
                        Timer::after_millis(20).await;
                        embassy_nrf::pac::POWER
                            .gpregret()
                            .write(|value| value.set_gpregret(0x4e));
                        cortex_m::peripheral::SCB::sys_reset();
                    }
                    if at_boundary && (packet == b"status\n" || packet == b"status\r\n") {
                        if host.write_all(online).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    // The executive's own account of the radio: what actually armed, what
                    // actually arrived, and which arm the unattended wait woke on. This is
                    // the probe that distinguishes "silently dead path" from "nothing to
                    // hear", which no other surface can.
                    if at_boundary && (packet == b"air\n" || packet == b"air\r\n") {
                        let d = exec.diag();
                        let mut reply = radio_face::Text::<176>::empty();
                        let _ = write!(
                            &mut reply,
                            "air region={} duty={}ms armed={} armfail={} rxok={} rxerr={} \
                             txok={} txerr={} noregion={} overduty={} beats={} frames={}\r\n",
                            exec.region().name(),
                            exec.duty_spent_ms(),
                            d.rx_armed,
                            d.rx_arm_failed,
                            d.rx_ok,
                            d.rx_err,
                            d.tx_ok,
                            d.tx_err,
                            d.tx_no_region,
                            d.tx_over_duty,
                            d.wait_beats,
                            d.wait_frames,
                        );
                        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    // Live allocation. The boot line reports it once, when it is zero by
                    // construction; this reports it whenever asked, which is what the heltec
                    // doc's heap high-water done condition actually needs now that the node
                    // channel allocates.
                    if at_boundary && (packet == b"heap\n" || packet == b"heap\r\n") {
                        let mut reply = radio_face::Text::<48>::empty();
                        let _ = write!(
                            &mut reply,
                            "heap={}/{} free={}\r\n",
                            heap::used(),
                            heap::HEAP_SIZE,
                            heap::free(),
                        );
                        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    // Region selection: the same persist-and-reboot shape as the channel,
                    // because the boot carrier and the clamp both derive from it.
                    if at_boundary && let Some(probe) = region_probe(packet) {
                        let mut reboot = false;
                        let mut reply = radio_face::Text::<64>::empty();
                        match (settings, probe) {
                            (None, _) => {
                                let _ = write!(&mut reply, "region unavailable: no identity\r\n");
                            }
                            (Some(current), RegionProbe::Report) => {
                                let _ = write!(&mut reply, "region={}\r\n", current.region.name());
                            }
                            (Some(current), RegionProbe::Set(wanted)) => {
                                let next = Settings {
                                    region: wanted,
                                    ..current
                                };
                                match exec.save_settings(&next) {
                                    Ok(()) => {
                                        reboot = true;
                                        let _ = write!(
                                            &mut reply,
                                            "region={}; rebooting\r\n",
                                            wanted.name()
                                        );
                                    }
                                    Err(_) => {
                                        let _ = write!(&mut reply, "region write failed\r\n");
                                    }
                                }
                            }
                        }
                        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
                            break;
                        }
                        if reboot {
                            Timer::after_millis(250).await;
                            cortex_m::peripheral::SCB::sys_reset();
                        }
                        continue;
                    }
                    // Channel selection. Switching is by reboot, so this persists the choice
                    // and resets; the flash write lands at a moment nothing is listening,
                    // which is what keeps it clear of the radio-quiet rule.
                    if at_boundary && let Some(probe) = channel_probe(packet) {
                        let mut reboot = false;
                        let reply = match (settings, probe) {
                            (None, _) => &b"channel unavailable: no identity\r\n"[..],
                            (Some(current), ChannelProbe::Report) => match current.channel {
                                BootChannel::Modem => &b"channel=modem\r\n"[..],
                                BootChannel::Node => &b"channel=node\r\n"[..],
                            },
                            (Some(current), ChannelProbe::Set(wanted)) => {
                                let next = Settings {
                                    channel: wanted,
                                    ..current
                                };
                                match exec.save_settings(&next) {
                                    Ok(()) => {
                                        reboot = true;
                                        &b"channel set; rebooting\r\n"[..]
                                    }
                                    Err(_) => &b"channel write failed\r\n"[..],
                                }
                            }
                        };
                        if host.write_all(reply).await.is_err() {
                            break;
                        }
                        if reboot {
                            // Long enough for the reply to leave the USB endpoint. The
                            // bootloader probe's 20 ms is not: it truncated this line at
                            // thirteen bytes, because a CDC write returning only means the
                            // packet was queued.
                            Timer::after_millis(250).await;
                            cortex_m::peripheral::SCB::sys_reset();
                        }
                        continue;
                    }
                    if at_boundary && (packet == b"sync\n" || packet == b"sync\r\n") {
                        let sync = sx126x_sync_word(MESHTASTIC_SYNC_WORD);
                        let reply = if sync == [0x24, 0xb4] {
                            b"2b 24b4\r\n".as_slice()
                        } else {
                            b"sync encoding fault\r\n".as_slice()
                        };
                        if host.write_all(reply).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    if at_boundary && (packet == b"radio\n" || packet == b"radio\r\n") {
                        let reply = exec.diagnostics(&Sx126xDiagnostics).await;
                        if host.write_all(&reply).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    if at_boundary && (packet == b"ui\n" || packet == b"ui\r\n") {
                        let diagnostic = ui::diagnostic();
                        let mut reply = radio_face::Text::<96>::empty();
                        let _ = write!(
                            &mut reply,
                            "ui={}; display={}; screen={}; button={}; host={}; tft=write-only\r\n",
                            diagnostic.state,
                            diagnostic.display,
                            diagnostic.screen,
                            diagnostic.button,
                            diagnostic.host,
                        );
                        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    #[cfg(feature = "ui-bench")]
                    if at_boundary && (packet == b"fault\n" || packet == b"fault\r\n") {
                        publish_fault(exec.status_mut(), 0xfe, "BENCH FAULT");
                        if host.write_all(b"ui bench fault set\r\n").await.is_err() {
                            break;
                        }
                        continue;
                    }
                    #[cfg(feature = "ui-bench")]
                    if at_boundary && (packet == b"clear\n" || packet == b"clear\r\n") {
                        publish_online(exec.status_mut());
                        if host.write_all(b"ui bench fault cleared\r\n").await.is_err() {
                            break;
                        }
                        continue;
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
