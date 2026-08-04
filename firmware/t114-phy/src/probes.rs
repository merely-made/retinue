//! The board's text probes: the bench vocabulary, in one place.
//!
//! Everything a host can ask the board over plain text — status, radio diagnostics, heap,
//! crash residue, the persisted region and channel — lives here rather than in `main`'s
//! loop, so the loop stays a loop. Probes are only recognised at a frame boundary; the
//! caller says where that is, because only the channel knows its parser's state.

use core::fmt::Write as _;

use embassy_time::Timer;
use radio_hand::executive::ChipDiagnostics;
use radio_hand::executive::Executive;
use radio_hand::link::HostLink;
use radio_hand::settings::{Channel as BootChannel, Settings};
use selvage::{MESHTASTIC_SYNC_WORD, sx126x_sync_word};

use crate::{ChannelProbe, RegionProbe, channel_probe, crash, heap, region_probe, ui};

/// What a batch of host bytes turned out to be.
pub enum Outcome {
    /// Not a probe; hand the bytes to the channel.
    NotAProbe,
    /// A probe was answered; take the next batch.
    Served,
    /// The host vanished mid-reply; end the session.
    HostGone,
}

/// Answer a board probe, or say it was not one.
pub async fn handle<L, RK, DLY, D>(
    packet: &[u8],
    at_boundary: bool,
    online: &radio_face::Text<192>,
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
    if at_boundary && (packet == b"status\n" || packet == b"status\r\n") {
        if host.write_all(online.as_str().as_bytes()).await.is_err() {
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // The executive's own account of the radio: what actually armed, what
    // actually arrived, and which arm the unattended wait woke on. This is
    // the probe that distinguishes "silently dead path" from "nothing to
    // hear", which no other surface can.
    // Listen-before-talk, on or off. Runtime only, so every boot is a good citizen; the
    // bench asks for the comparison each time it wants one.
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
    if at_boundary && (packet == b"air\n" || packet == b"air\r\n") {
        let d = exec.diag();
        let mut reply = radio_face::Text::<256>::empty();
        let _ = write!(
            &mut reply,
            "air region={} duty={}ms listen={} armed={} armfail={} rxok={} rxerr={} \
             txok={} txerr={} noregion={} overduty={} cadclear={} cadbusy={} \
             cadgiveup={} cadover={} cadfault={} beats={} frames={}\r\n",
            exec.region().name(),
            exec.duty_spent_ms(),
            if exec.listen_first() { "on" } else { "off" },
            d.rx_armed,
            d.rx_arm_failed,
            d.rx_ok,
            d.rx_err,
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
        return Outcome::Served;
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
            return Outcome::HostGone;
        }
        return Outcome::Served;
    }
    // The crash residue: what the last bad boot left behind. `crash clear`
    // forgets it; the count also decays after a clean minute on its own.
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
    // Bench hooks for the supervised-reboot receipts. Deliberate and
    // undisguised: `crashtest` proves the panic path (residue, reboot,
    // fallback after three), `hangtest` proves the watchdog. A host that
    // can reach these can already reboot the board via `bootloader`.
    if at_boundary && (packet == b"crashtest\n" || packet == b"crashtest\r\n") {
        let _ = host.write_all(b"panicking now\r\n").await;
        Timer::after_millis(100).await;
        panic!("deliberate crashtest");
    }
    if at_boundary && (packet == b"hangtest\n" || packet == b"hangtest\r\n") {
        let _ = host.write_all(b"hanging now\r\n").await;
        Timer::after_millis(100).await;
        // A busy loop that never yields: the executor starves, the petting
        // stops, and the watchdog does its one job.
        #[allow(clippy::empty_loop)]
        loop {}
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
                        let _ = write!(&mut reply, "region={}; rebooting\r\n", wanted.name());
                    }
                    Err(_) => {
                        let _ = write!(&mut reply, "region write failed\r\n");
                    }
                }
            }
        }
        // The reply is a courtesy; the reboot is the contract. By this point the settings
        // are already committed to flash, so the board must come back on them — a host that
        // vanished mid-reply must not leave it running on state it no longer has, with a
        // page erase already spent outside any quiet window.
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
        // Same contract as the region probe: committed settings mean the reboot happens
        // whether or not the host is still there to be told.
        let reported = host.write_all(reply).await;
        if reboot {
            // Long enough for the reply to leave the USB endpoint. The
            // bootloader probe's 20 ms is not: it truncated this line at
            // thirteen bytes, because a CDC write returning only means the
            // packet was queued.
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
