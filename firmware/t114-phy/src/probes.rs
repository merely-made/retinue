//! The board's plain-text bench probes: status, radio diagnostics, heap, crash residue,
//! region and channel. Recognised only at a frame boundary, which the caller reports
//! because only the channel knows its parser's state.

use core::fmt::Write as _;

use embassy_time::Timer;
use radio_hand::executive::{ChipDiagnostics, Executive};
use radio_hand::link::HostLink;
use radio_hand::profiles::{DetectionProfileId, ReceiveProfileId};
use radio_hand::region::Region;
use radio_hand::settings::{Channel as BootChannel, Settings};
use selvage::{MESHTASTIC_SYNC_WORD, sx126x_sync_word};

use crate::{crash, heap, lxmf, ui};
#[cfg(feature = "ui-bench")]
use crate::{publish_fault, publish_online};
use le3::{serve_le3_cad, serve_le3_plan, serve_le3_rx};

mod le3;

/// What a batch of host bytes turned out to be.
pub enum Outcome {
    /// Not a probe; hand the bytes to the channel.
    NotAProbe,
    /// A probe was answered; take the next batch.
    Served,
    /// The host vanished mid-reply; end the session.
    HostGone,
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
        b"channel rnode" => Some(ChannelProbe::Set(BootChannel::Rnode)),
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

/// Read a host line as a region probe. Names match the region table case-insensitively.
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

/// Answer a board probe, or say it was not one.
pub async fn handle<L, RK, DLY, D>(
    packet: &[u8],
    at_boundary: bool,
    online: &radio_face::Text<320>,
    settings: Option<Settings>,
    diagnostics: &D,
    exec: &mut Executive<'_, RK, DLY>,
    host: &mut L,
) -> Outcome
where
    L: HostLink,
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
    D: ChipDiagnostics<RK, DLY>,
{
    if at_boundary && (packet == b"bootloader\n" || packet == b"bootloader\r\n") {
        let _ = host.write_all(b"entering serial bootloader\r\n").await;
        Timer::after_millis(20).await;
        embassy_nrf::pac::POWER
            .gpregret()
            .write(|value| value.set_gpregret(0x4e));
        cortex_m::peripheral::SCB::sys_reset();
    }
    if at_boundary && (packet == b"flashcounts\n" || packet == b"flashcounts\r\n") {
        let (erases, writes) = crate::store::flash_attempts();
        let mut reply = radio_face::Text::<96>::empty();
        let _ = write!(
            &mut reply,
            "flashcounts erases={erases} writes={writes}\r\n"
        );
        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"status\n" || packet == b"status\r\n") {
        if host.write_all(online.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // Listen-before-talk toggle. Runtime only, never persisted.
    if at_boundary && (packet == b"cad on\n" || packet == b"cad on\r\n") {
        exec.set_listen_first(true);
        if host.write_all(b"listen=on\r\n").await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"cad off\n" || packet == b"cad off\r\n") {
        exec.set_listen_first(false);
        if host.write_all(b"listen=off\r\n").await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // The executive's radio counters: tells a silently dead path from nothing to hear.
    if at_boundary && (packet == b"air\n" || packet == b"air\r\n") {
        let d = exec.diag();
        let mut reply = radio_face::Text::<256>::empty();
        let _ = write!(
            &mut reply,
            "air region={} duty={}ms listen={} armed={} armfail={} rxok={} rxerr={} \
             rxbad={} txok={} txerr={} noregion={} overduty={} cadclear={} cadbusy={} \
             cadgiveup={} cadover={} cadfault={} beats={} frames={}\r\n",
            exec.region().name(),
            exec.duty_spent_ms(),
            if exec.listen_first() { "on" } else { "off" },
            d.rx_armed,
            d.rx_arm_failed,
            d.rx_ok,
            d.rx_err,
            d.rx_damaged,
            d.tx_ok,
            d.tx_err,
            d.tx_no_region,
            d.tx_over_duty,
            d.cad_clear,
            d.cad_busy,
            d.tx_channel_busy,
            d.cad_override,
            d.cad_fault,
            d.wait_beats,
            d.wait_frames,
        );
        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        let mut scan = radio_face::Text::<192>::empty();
        let _ = write!(
            &mut scan,
            "scan cad1={}/{} cad2={}/{} rx1={}/{} rx2={}/{} rx3={}/{}\r\n",
            d.scan_cad_hits[0],
            d.scan_cad_misses[0],
            d.scan_cad_hits[1],
            d.scan_cad_misses[1],
            d.scan_rx_captures[0],
            d.scan_rx_misses[0],
            d.scan_rx_captures[1],
            d.scan_rx_misses[1],
            d.scan_rx_captures[2],
            d.scan_rx_misses[2],
        );
        if host.write_all(scan.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"le3 plan\n" || packet == b"le3 plan\r\n") {
        return serve_le3_plan(exec, host).await;
    }
    if at_boundary && (packet == b"le3 cad 1\n" || packet == b"le3 cad 1\r\n") {
        return serve_le3_cad(DetectionProfileId(1), exec, host).await;
    }
    if at_boundary && (packet == b"le3 cad 2\n" || packet == b"le3 cad 2\r\n") {
        return serve_le3_cad(DetectionProfileId(2), exec, host).await;
    }
    if at_boundary && (packet == b"le3 rx 1\n" || packet == b"le3 rx 1\r\n") {
        return serve_le3_rx(ReceiveProfileId(1), exec, host).await;
    }
    if at_boundary && (packet == b"le3 rx 2\n" || packet == b"le3 rx 2\r\n") {
        return serve_le3_rx(ReceiveProfileId(2), exec, host).await;
    }
    if at_boundary && (packet == b"le3 rx 3\n" || packet == b"le3 rx 3\r\n") {
        return serve_le3_rx(ReceiveProfileId(3), exec, host).await;
    }
    // LXMF codec and stamp checks against captured stock answers, with their cost. One line
    // per probe: stamp work takes seconds, and hosts read to the first newline.
    if at_boundary && (packet == b"lxmf\n" || packet == b"lxmf\r\n") {
        let mut reply = radio_face::Text::<256>::empty();
        lxmf::check_codec(&mut reply);
        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"lxmf stamp\n" || packet == b"lxmf stamp\r\n") {
        let mut reply = radio_face::Text::<256>::empty();
        lxmf::check_stamp(&mut reply).await;
        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"lxmf mint\n" || packet == b"lxmf mint\r\n") {
        let mut reply = radio_face::Text::<256>::empty();
        lxmf::check_mint(&mut reply).await;
        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // Live and peak allocation; the high-water mark survives buffers being released.
    if at_boundary && (packet == b"heap\n" || packet == b"heap\r\n") {
        let mut reply = radio_face::Text::<64>::empty();
        let _ = write!(
            &mut reply,
            "heap={}/{} highwater={} free={}\r\n",
            heap::used(),
            heap::HEAP_SIZE,
            heap::high_water(),
            heap::free(),
        );
        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // Crash residue. The count also decays after a clean minute.
    if at_boundary && (packet == b"crash\n" || packet == b"crash\r\n") {
        let (count, msg) = crash::residue();
        let mut reply = radio_face::Text::<160>::empty();
        let _ = write!(
            &mut reply,
            "crash count={} msg={}\r\n",
            count,
            core::str::from_utf8(msg).unwrap_or("?"),
        );
        if host.write_all(reply.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"crash clear\n" || packet == b"crash clear\r\n") {
        crash::clear_all();
        if host.write_all(b"crash cleared\r\n").await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // Supervised-reboot bench hooks: `crashtest` exercises the panic path, `hangtest` the
    // watchdog. A host that reaches these can already reboot via `bootloader`.
    if at_boundary && (packet == b"crashtest\n" || packet == b"crashtest\r\n") {
        let _ = host.write_all(b"panicking now\r\n").await;
        Timer::after_millis(100).await;
        panic!("deliberate crashtest");
    }
    if at_boundary && (packet == b"hangtest\n" || packet == b"hangtest\r\n") {
        let _ = host.write_all(b"hanging now\r\n").await;
        Timer::after_millis(100).await;
        // Never yields, so the watchdog stops being petted.
        #[allow(clippy::empty_loop)]
        loop {}
    }
    // Region: persist and reboot, since the boot carrier and clamp derive from it.
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
                        let _ = write!(&mut reply, "region={}; rebooting\r\n", wanted.name());
                    }
                    Err(_) => {
                        let _ = write!(&mut reply, "region write failed\r\n");
                    }
                }
            }
        }
        // Settings are committed, so reboot even if the host vanished mid-reply.
        let reported = host.write_all(reply.as_str().as_bytes()).await;
        if reboot {
            Timer::after_millis(250).await;
            cortex_m::peripheral::SCB::sys_reset();
        }
        if reported.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // Channel: persist and reboot. The flash write lands while nothing is listening, which
    // keeps it clear of the radio-quiet rule.
    if at_boundary && let Some(probe) = channel_probe(packet) {
        let mut reboot = false;
        let reply = match (settings, probe) {
            (None, _) => &b"channel unavailable: no identity\r\n"[..],
            (Some(current), ChannelProbe::Report) => match current.channel {
                BootChannel::Modem => &b"channel=modem\r\n"[..],
                BootChannel::LegacyNode => &b"channel=node state=node-unarmed\r\n"[..],
                BootChannel::Node => &b"channel=node\r\n"[..],
                BootChannel::Rnode => &b"channel=rnode\r\n"[..],
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
        // As for region: committed settings always reboot.
        let reported = host.write_all(reply).await;
        if reboot {
            // Lets the reply leave the endpoint: 20 ms truncated it, as a CDC write
            // returning only means the packet was queued.
            Timer::after_millis(250).await;
            cortex_m::peripheral::SCB::sys_reset();
        }
        if reported.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"sync\n" || packet == b"sync\r\n") {
        let sync = sx126x_sync_word(MESHTASTIC_SYNC_WORD);
        let reply = if sync == [0x24, 0xb4] {
            b"2b 24b4\r\n".as_slice()
        } else {
            b"sync encoding fault\r\n".as_slice()
        };
        if host.write_all(reply).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    if at_boundary && (packet == b"radio\n" || packet == b"radio\r\n") {
        let reply = exec.diagnostics(diagnostics).await;
        if host.write_all(&reply).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
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
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    #[cfg(feature = "ui-bench")]
    if at_boundary && (packet == b"fault\n" || packet == b"fault\r\n") {
        publish_fault(exec.status_mut(), 0xfe, "BENCH FAULT");
        if host.write_all(b"ui bench fault set\r\n").await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    #[cfg(feature = "ui-bench")]
    if at_boundary && (packet == b"clear\n" || packet == b"clear\r\n") {
        publish_online(exec.status_mut());
        if host.write_all(b"ui bench fault cleared\r\n").await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }

    Outcome::NotAProbe
}
