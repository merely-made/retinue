//! Channel selection and the RNode serve loop, plus the text probes both loops share.
//!
//! The V4 keeps its bespoke modem loop in `main` because the low-power work needs its own
//! hand on the radio. The channel selector therefore lives beside it rather than replacing
//! it: the persisted channel byte picks which loop the boot enters, switching is by reboot,
//! and the probe vocabulary is the T114's exactly, because a bench should not have to
//! remember which board it is talking to.

use embassy_futures::select::{Either, select};
use lora_phy::mod_params::RadioError;
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa, RxMode};
use radio_hand::channel::rnode::RNodeChannel;
use radio_hand::channel::{Channel as _, ChannelInfo as _, Event};
use radio_hand::executive::{Executive, Face, RadioState};
use radio_hand::link::HostLink;
use radio_hand::region::Region;
use radio_hand::settings::{Channel as BootChannel, Settings};

use crate::{power, store, ui};

/// What a `region` line asked for: report, or set-and-reboot.
pub fn region_probe(packet: &[u8]) -> Option<Option<Region>> {
    let line = packet
        .strip_suffix(b"\r\n")
        .or_else(|| packet.strip_suffix(b"\n"))?;
    if line == b"region" {
        return Some(None);
    }
    let name = line.strip_prefix(b"region ")?;
    Region::choices()
        .find(|region| region.name().as_bytes().eq_ignore_ascii_case(name))
        .map(Some)
}

/// What a `channel` line asked for.
pub enum ChannelProbe {
    Report,
    Set(BootChannel),
    /// A channel the vocabulary knows but this board cannot serve (`channel node` needs an
    /// allocator the V4 does not carry). Answered rather than ignored, so the line does not
    /// fall through into whatever the active channel parses.
    Unavailable,
}

pub fn channel_probe(packet: &[u8]) -> Option<ChannelProbe> {
    let line = packet
        .strip_suffix(b"\r\n")
        .or_else(|| packet.strip_suffix(b"\n"))?;
    match line {
        b"channel" => Some(ChannelProbe::Report),
        b"channel modem" => Some(ChannelProbe::Set(BootChannel::Modem)),
        b"channel rnode" => Some(ChannelProbe::Set(BootChannel::Rnode)),
        b"channel node" => Some(ChannelProbe::Unavailable),
        _ => None,
    }
}

/// What a batch of host bytes turned out to be.
pub enum Outcome {
    NotAProbe,
    Served,
}

/// Answer the probes both channels share: `status`, `sync`, `ui`, `region`, `channel`.
///
/// `region` and `channel` persist and reset. The reply is a courtesy and the reboot is the
/// contract, same as the T114: once the settings are committed the board must come back on
/// them, whether or not the host read the reply.
pub async fn probe<L: HostLink>(
    packet: &[u8],
    online: &'static [u8],
    identity_line: &[u8],
    settings: Option<Settings>,
    store: &mut store::SettingsStore,
    host: &mut L,
) -> Outcome {
    if packet == b"status\n" || packet == b"status\r\n" {
        let _ = host.write_all(online).await;
        let _ = host.write_all(identity_line).await;
        return Outcome::Served;
    }
    if packet == b"sync\n" || packet == b"sync\r\n" {
        let _ = host.write_all(b"2b 24b4\r\n").await;
        return Outcome::Served;
    }
    if packet == b"ui\n" || packet == b"ui\r\n" {
        use core::fmt::Write as _;
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
        return Outcome::Served;
    }
    if let Some(wanted) = region_probe(packet) {
        use core::fmt::Write as _;
        let mut reboot = false;
        let mut reply = radio_face::Text::<64>::empty();
        match (settings, wanted) {
            (None, _) => {
                let _ = write!(&mut reply, "region unavailable: no identity\r\n");
            }
            (Some(current), None) => {
                let _ = write!(&mut reply, "region={}\r\n", current.region.name());
            }
            (Some(current), Some(region)) => {
                let next = Settings { region, ..current };
                match store.save(&next) {
                    Ok(_) => {
                        reboot = true;
                        let _ = write!(&mut reply, "region={}; rebooting\r\n", region.name());
                    }
                    Err(_) => {
                        let _ = write!(&mut reply, "region write failed\r\n");
                    }
                }
            }
        }
        let _ = host.write_all(reply.as_str().as_bytes()).await;
        if reboot {
            embassy_time::Timer::after_millis(250).await;
            esp_hal::system::software_reset();
        }
        return Outcome::Served;
    }
    if let Some(wanted) = channel_probe(packet) {
        let mut reboot = false;
        let reply = match (settings, wanted) {
            (None, _) => &b"channel unavailable: no identity\r\n"[..],
            (_, ChannelProbe::Unavailable) => &b"channel node unavailable on this board\r\n"[..],
            (Some(current), ChannelProbe::Report) => match current.channel {
                BootChannel::Modem => &b"channel=modem\r\n"[..],
                BootChannel::Node => &b"channel=node\r\n"[..],
                BootChannel::Rnode => &b"channel=rnode\r\n"[..],
            },
            (Some(current), ChannelProbe::Set(channel)) => {
                let next = Settings { channel, ..current };
                match store.save(&next) {
                    Ok(_) => {
                        reboot = true;
                        &b"channel set; rebooting\r\n"[..]
                    }
                    Err(_) => &b"channel write failed\r\n"[..],
                }
            }
        };
        let _ = host.write_all(reply).await;
        if reboot {
            embassy_time::Timer::after_millis(250).await;
            esp_hal::system::software_reset();
        }
        return Outcome::Served;
    }
    Outcome::NotAProbe
}

/// The board as an RNode, forever.
///
/// Takes ownership of everything it needs because it never returns: switching is by reboot,
/// and this board's transports cannot detect a departing host, so there is no session to
/// end. No banner is written and no fault text goes to the host — a host that opened this
/// port expects KISS frames from the first byte, and text in that stream is corruption.
/// Faults still reach the screen.
pub async fn serve_rnode<RK, DLY, L>(
    mut lora: LoRa<RK, DLY>,
    mut radio: RadioState,
    mut local_status: radio_face::LocalStatus,
    face: Face,
    mut store: store::SettingsStore,
    settings: Option<Settings>,
    online: &'static [u8],
    identity_line: &[u8],
    mut host: L,
) -> !
where
    RK: RadioKind,
    DLY: DelayNs,
    L: HostLink,
{
    let region = settings.map(|s| s.region).unwrap_or_default();
    let mut channel = RNodeChannel::new();

    loop {
        if radio.prepare_rx {
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
                embassy_time::Timer::after_millis(250).await;
                continue;
            }
            local_status.radio = radio_face::RadioState::Online;
            local_status.fault = None;
            ui::publish(local_status, radio_face::LedSignal::Idle);
            radio.prepare_rx = false;
        }

        let mut host_packet = [0_u8; 64];
        let mut radio_frame = [0_u8; selvage::MAX_RADIO_FRAME_LEN];
        let outcome = select(
            host.read(&mut host_packet),
            lora.rx(&radio.rx, &mut radio_frame),
        );
        let outcome = if channel.at_boundary() {
            outcome.await
        } else {
            let _awake = power::Awake::new();
            outcome.await
        };
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
                let mut exec = Executive::new(
                    &mut lora,
                    &mut radio,
                    &mut local_status,
                    &face,
                    &mut store,
                    region,
                );
                let _ = channel
                    .serve(
                        &mut exec,
                        &mut host,
                        Event::RadioFrame {
                            frame: &radio_frame[..length],
                            rssi: packet_status.rssi,
                            snr: packet_status.snr,
                        },
                    )
                    .await;
            }
            // A packet that failed its CRC. The air's fault, not the radio's: dropped and
            // the receiver is still listening, so there is nothing to repair.
            Either::Second(Err(RadioError::PayloadCrcError)) => {}
            Either::Second(Err(_)) => {
                local_status.radio = radio_face::RadioState::Fault;
                local_status.fault = Some(radio_face::Fault {
                    code: 6,
                    message: radio_face::Text::from_truncated("RADIO RX"),
                });
                ui::publish(local_status, radio_face::LedSignal::Idle);
                radio.prepare_rx = true;
            }
            Either::First(Err(_)) | Either::First(Ok(0)) => {}
            Either::First(Ok(length)) => {
                local_status.host = radio_face::HostState::Attached;
                local_status.last_wake = radio_face::WakeSource::Host;
                ui::publish(local_status, radio_face::LedSignal::Idle);
                let packet = &host_packet[..length];
                if channel.at_boundary()
                    && matches!(
                        probe(
                            packet,
                            online,
                            identity_line,
                            settings,
                            &mut store,
                            &mut host
                        )
                        .await,
                        Outcome::Served
                    )
                {
                    continue;
                }
                let mut exec = Executive::new(
                    &mut lora,
                    &mut radio,
                    &mut local_status,
                    &face,
                    &mut store,
                    region,
                );
                let _ = channel
                    .serve(&mut exec, &mut host, Event::HostBytes(packet))
                    .await;
            }
        }
    }
}
