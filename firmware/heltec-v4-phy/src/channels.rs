//! Channel selection and the RNode serve loop, plus the text probes both loops share.
//!
//! The V4 keeps its bespoke modem loop in `main` because the low-power work needs its own
//! sleep policy. The channel selector therefore lives beside it rather than replacing
//! it: the persisted channel byte picks which loop the boot enters, switching is by reboot,
//! and the probe vocabulary is the T114's exactly, because a bench should not have to
//! remember which board it is talking to.

use embassy_futures::select::{Either, select};
use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use radio_hand::announce_reservation::DEFAULT_LEASE;
use radio_hand::channel::rnode::RNodeChannel;
use radio_hand::channel::{Channel as _, ChannelInfo as _, Event};
use radio_hand::link::HostLink;
use radio_hand::region::Region;
use radio_hand::settings::{Channel as BootChannel, Settings};

use crate::{power, radio_owner::V4RadioOwner, store, ui};

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

fn timebase_reserve_lease(packet: &[u8]) -> Option<Result<u64, ()>> {
    let line = packet
        .strip_suffix(b"\r\n")
        .or_else(|| packet.strip_suffix(b"\n"))?;
    let argument = line.strip_prefix(b"timebase reserve")?;
    if argument.is_empty() {
        return Some(Ok(DEFAULT_LEASE));
    }
    let digits = argument.strip_prefix(b" ")?;
    if digits.is_empty() {
        return Some(Err(()));
    }
    let mut value = 0_u64;
    for byte in digits {
        if !byte.is_ascii_digit() {
            return Some(Err(()));
        }
        let digit = u64::from(byte - b'0');
        value = match value
            .checked_mul(10)
            .and_then(|value| value.checked_add(digit))
        {
            Some(value) => value,
            None => return Some(Err(())),
        };
    }
    Some(Ok(value))
}

fn timebase_probe(packet: &[u8]) -> Option<TimebaseProbe> {
    let line = packet
        .strip_suffix(b"\r\n")
        .or_else(|| packet.strip_suffix(b"\n"))?;
    if line == b"timebase" {
        return Some(TimebaseProbe::Report);
    }
    timebase_reserve_lease(packet).map(|lease| TimebaseProbe::Reserve(lease))
}

enum TimebaseProbe {
    Report,
    Reserve(Result<u64, ()>),
}

fn decimal(mut value: u64, out: &mut [u8; 20]) -> usize {
    let mut len = 0;
    loop {
        out[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    out[..len].reverse();
    len
}

/// Answer the probes both channels share: `status`, `sync`, `ui`, `region`, `channel`, and
/// the V4's storage-only `timebase` report/reservation probe. The on-air successor gate remains
/// open: this board still has no native Node channel, so a verified lease is an inspectable
/// persistence artifact rather than permission for the modem to emit announces.
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
    if let Some(wanted) = timebase_probe(packet) {
        use core::fmt::Write as _;
        let mut reply = radio_face::Text::<96>::empty();
        match wanted {
            TimebaseProbe::Report => match store.announce_reservation_state() {
                Ok(state) => match state.through() {
                    Some(through) => {
                        let mut digits = [0_u8; 20];
                        let len = decimal(through, &mut digits);
                        let _ = write!(
                            &mut reply,
                            "timebase through={}\r\n",
                            core::str::from_utf8(&digits[..len]).unwrap_or("?")
                        );
                    }
                    None => {
                        let _ = write!(&mut reply, "timebase uncommissioned\r\n");
                    }
                },
                Err(_) => {
                    let _ = write!(&mut reply, "timebase unavailable: reservation corrupt\r\n");
                }
            },
            TimebaseProbe::Reserve(Err(())) => {
                let _ = write!(&mut reply, "timebase reserve invalid lease\r\n");
            }
            TimebaseProbe::Reserve(Ok(lease)) => match store.reserve_announce_lease(lease) {
                Ok(active) => {
                    let mut floor = [0_u8; 20];
                    let mut through = [0_u8; 20];
                    let floor_len = decimal(active.floor(), &mut floor);
                    let through_len = decimal(active.reserved_through(), &mut through);
                    let _ = write!(
                        &mut reply,
                        "timebase reserved floor={} through={}; rebooting\r\n",
                        core::str::from_utf8(&floor[..floor_len]).unwrap_or("?"),
                        core::str::from_utf8(&through[..through_len]).unwrap_or("?"),
                    );
                    let _ = host.write_all(reply.as_str().as_bytes()).await;
                    embassy_time::Timer::after_millis(250).await;
                    esp_hal::system::software_reset();
                }
                Err(_) => {
                    let _ = write!(&mut reply, "timebase reserve failed\r\n");
                }
            },
        }
        let _ = host.write_all(reply.as_str().as_bytes()).await;
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
                BootChannel::LegacyNode | BootChannel::Node => {
                    &b"channel=node unavailable on this board\r\n"[..]
                }
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
    mut owner: V4RadioOwner<RK, DLY>,
    online: &'static [u8],
    identity_line: &[u8],
    mut host: L,
) -> !
where
    RK: RadioKind,
    DLY: DelayNs,
    L: HostLink,
{
    let mut channel = RNodeChannel::new();

    loop {
        {
            let _awake = power::Awake::new();
            match owner.ensure_rx().await {
                Ok(true) => owner.radio_online(),
                Ok(false) => {}
                Err(crate::radio_owner::RxSetupFault::Prepare) => {
                    owner.radio_fault(5, "RX SETUP");
                    embassy_time::Timer::after_millis(250).await;
                    continue;
                }
                Err(crate::radio_owner::RxSetupFault::Arm) => {
                    owner.radio_fault(5, "RX ARM");
                    embassy_time::Timer::after_millis(250).await;
                    continue;
                }
            }
        }

        let mut host_packet = [0_u8; 64];
        let mut radio_frame = [0_u8; selvage::MAX_RADIO_FRAME_LEN];
        // The radio is already in continuous receive. Race only the interrupt wait. Once it
        // wins, the frame remains in the chip until the un-raced collection below completes.
        let outcome = select(host.read(&mut host_packet), owner.wait_rx_irq());
        let outcome = if channel.at_boundary() {
            outcome.await
        } else {
            let _awake = power::Awake::new();
            outcome.await
        };
        let _awake = power::Awake::new();
        match outcome {
            Either::Second(Ok(())) => {
                // Deliberately not raced: the IRQ has already been consumed and the frame is in
                // the radio until it is read out. A preamble-only IRQ is reported as pending;
                // continuous RX remains armed and the outer loop waits for the next IRQ.
                let Some(frame) = (match owner.collect(&mut radio_frame).await {
                    Ok(frame) => frame,
                    Err(_) => {
                        owner.radio_fault(6, "RADIO RX");
                        continue;
                    }
                }) else {
                    continue;
                };
                owner.note_radio_frame(&frame);
                let mut exec = owner.executive();
                let _ = channel
                    .serve(
                        &mut exec,
                        &mut host,
                        Event::RadioFrame {
                            frame: &radio_frame[..frame.len],
                            rssi: frame.rssi,
                            snr: frame.snr,
                        },
                    )
                    .await;
            }
            Either::Second(Err(_)) => {
                owner.radio_fault(6, "RADIO RX");
            }
            Either::First(Err(_)) | Either::First(Ok(0)) => {}
            Either::First(Ok(length)) => {
                owner.note_host_activity();
                let packet = &host_packet[..length];
                if channel.at_boundary()
                    && matches!(
                        owner.probe(packet, online, identity_line, &mut host).await,
                        Outcome::Served
                    )
                {
                    continue;
                }
                let mut exec = owner.executive();
                let _ = channel
                    .serve(&mut exec, &mut host, Event::HostBytes(packet))
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timebase_probe_defaults_to_the_shared_lease() {
        assert!(matches!(
            timebase_probe(b"timebase reserve\n"),
            Some(TimebaseProbe::Reserve(Ok(DEFAULT_LEASE)))
        ));
    }

    #[test]
    fn timebase_probe_accepts_a_decimal_lease_and_rejects_bad_input() {
        assert!(matches!(
            timebase_probe(b"timebase reserve 123\r\n"),
            Some(TimebaseProbe::Reserve(Ok(123)))
        ));
        assert!(matches!(
            timebase_probe(b"timebase reserve nope\n"),
            Some(TimebaseProbe::Reserve(Err(())))
        ));
        assert!(matches!(
            timebase_probe(b"timebase reserve 123x\n"),
            Some(TimebaseProbe::Reserve(Err(())))
        ));
        assert!(timebase_probe(b"timebase reserver\n").is_none());
        assert!(matches!(
            timebase_probe(b"timebase\n"),
            Some(TimebaseProbe::Report)
        ));
    }

    #[test]
    fn decimal_is_small_and_deterministic() {
        let mut out = [0_u8; 20];
        let len = decimal(65_536, &mut out);
        assert_eq!(&out[..len], b"65536");
        let len = decimal(0, &mut out);
        assert_eq!(&out[..len], b"0");
    }
}
