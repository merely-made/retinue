//! The board's text probes: the bench vocabulary, in one place.
//!
//! Everything a host can ask the board over plain text — status, radio diagnostics, heap,
//! crash residue, the persisted region and channel — lives here rather than in `main`'s
//! loop, so the loop stays a loop. Probes are only recognised at a frame boundary; the
//! caller says where that is, because only the channel knows its parser's state.

use core::fmt::Write as _;

use embassy_time::{Duration, Instant, Timer, with_timeout};
use radio_hand::executive::{ChipDiagnostics, Executive, RadioFault};
use radio_hand::link::HostLink;
use radio_hand::profiles::{DetectionProfileId, ReceiveProfileId};
use radio_hand::settings::{Channel as BootChannel, Settings};
use selvage::{MESHTASTIC_SYNC_WORD, sx126x_sync_word};

use crate::{ChannelProbe, RegionProbe, channel_probe, crash, heap, le3, lxmf, region_probe, ui};

/// What a batch of host bytes turned out to be.
pub enum Outcome {
    /// Not a probe; hand the bytes to the channel.
    NotAProbe,
    /// A probe was answered; take the next batch.
    Served,
    /// The host vanished mid-reply; end the session.
    HostGone,
}

const LE3_CAD_TRIALS: u8 = 12;

async fn restore_profile<RK, DLY>(
    exec: &mut Executive<'_, RK, DLY>,
    profile: selvage::PhyProfile,
) -> bool
where
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    exec.apply_profile(&profile).await == selvage::CONFIG_ACCEPTED && exec.ensure_rx().await.is_ok()
}

async fn serve_le3_plan<L, RK, DLY>(exec: &mut Executive<'_, RK, DLY>, host: &mut L) -> Outcome
where
    L: HostLink,
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    let plan = match le3::plan(exec.profile().frequency_hz) {
        Ok(plan) => plan,
        Err(_) => {
            return if host.write_all(b"le3 plan invalid\r\n").await.is_err() {
                Outcome::HostGone
            } else {
                Outcome::Served
            };
        }
    };
    let (fits, overfull_rejected) = le3::budget_facts(&plan);
    let mut reply = radio_face::Text::<192>::empty();
    let _ = write!(
        &mut reply,
        "le3 plan detections={} receives={} steps={} dwell={}ms budget={}ms fits={} \
         overfull_rejected={} sequence=d1,r1-12,r2-2b,d2,r3-2b\r\n",
        plan.detection_count(),
        plan.receive_count(),
        plan.cycle_steps(),
        plan.cycle_dwell_ms(),
        le3::CYCLE_BUDGET_MS,
        u8::from(fits),
        u8::from(overfull_rejected),
    );
    if host.write_all(reply.as_str().as_bytes()).await.is_err() {
        Outcome::HostGone
    } else {
        Outcome::Served
    }
}

async fn serve_le3_cad<L, RK, DLY>(
    id: DetectionProfileId,
    exec: &mut Executive<'_, RK, DLY>,
    host: &mut L,
) -> Outcome
where
    L: HostLink,
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    let plan = match le3::plan(exec.profile().frequency_hz) {
        Ok(plan) => plan,
        Err(_) => return Outcome::Served,
    };
    let Some(profile) = le3::detection_phy(&plan, id) else {
        return Outcome::Served;
    };
    let previous = exec.profile();
    let mut ready = radio_face::Text::<128>::empty();
    let _ = write!(
        &mut ready,
        "le3 cad ready id={} sf={} bw={} trials={} lead=300ms\r\n",
        id.0, profile.spreading_factor, profile.bandwidth_hz, LE3_CAD_TRIALS,
    );
    if host.write_all(ready.as_str().as_bytes()).await.is_err() {
        return Outcome::HostGone;
    }
    Timer::after_millis(300).await;

    let mut hits = 0_u8;
    let mut misses = 0_u8;
    let mut faults = 0_u8;
    let mut apply_us = 0_u64;
    let mut retune_us = 0_u64;
    let mut cad_us = 0_u64;
    for _ in 0..LE3_CAD_TRIALS {
        match exec.observe_cad(&profile).await {
            Ok(observation) => {
                exec.note_scan_cad(id.0, observation.activity);
                if observation.activity {
                    hits = hits.saturating_add(1);
                } else {
                    misses = misses.saturating_add(1);
                }
                apply_us = apply_us.saturating_add(observation.apply_us);
                retune_us = retune_us.saturating_add(observation.retune_us);
                cad_us = cad_us.saturating_add(observation.cad_us);
            }
            Err(_) => faults = faults.saturating_add(1),
        }
        Timer::after_millis(25).await;
    }
    let restored = restore_profile(exec, previous).await;
    let measured = u64::from(hits) + u64::from(misses);
    let denominator = measured.max(1);
    let mut reply = radio_face::Text::<192>::empty();
    let _ = write!(
        &mut reply,
        "le3 cad id={} hits={} misses={} faults={} apply_avg={}us retune_avg={}us \
         cad_avg={}us symbols=8 restored={}\r\n",
        id.0,
        hits,
        misses,
        faults,
        apply_us / denominator,
        retune_us / denominator,
        cad_us / denominator,
        u8::from(restored),
    );
    if host.write_all(reply.as_str().as_bytes()).await.is_err() {
        Outcome::HostGone
    } else {
        Outcome::Served
    }
}

async fn serve_le3_rx<L, RK, DLY>(
    id: ReceiveProfileId,
    exec: &mut Executive<'_, RK, DLY>,
    host: &mut L,
) -> Outcome
where
    L: HostLink,
    RK: lora_phy::mod_traits::RadioKind,
    DLY: lora_phy::DelayNs,
{
    let plan = match le3::plan(exec.profile().frequency_hz) {
        Ok(plan) => plan,
        Err(_) => return Outcome::Served,
    };
    let Some((receive, profile)) = le3::receive_phy(&plan, id) else {
        return Outcome::Served;
    };
    let previous = exec.profile();
    let arm = match exec.arm_capture(&profile).await {
        Ok(arm) => arm,
        Err(_) => {
            let _ = restore_profile(exec, previous).await;
            return if host.write_all(b"le3 rx arm fault\r\n").await.is_err() {
                Outcome::HostGone
            } else {
                Outcome::Served
            };
        }
    };
    let mut ready = radio_face::Text::<128>::empty();
    let _ = write!(
        &mut ready,
        "le3 rx ready id={} sync={:02x} sf={} dwell={}ms\r\n",
        id.0, profile.sync_word, profile.spreading_factor, receive.capture_dwell_ms,
    );
    if host.write_all(ready.as_str().as_bytes()).await.is_err() {
        let _ = restore_profile(exec, previous).await;
        return Outcome::HostGone;
    }
    // Leave enough USB time for the ready marker to reach the host before the capture
    // window starts. Continuous RX is already armed, so a frame sent immediately remains
    // latched by DIO1 and is collected below.
    Timer::after_millis(50).await;
    let acquisition_started = Instant::now();
    let mut frame = [0_u8; 255];
    let capture = async {
        loop {
            exec.wait_rx_irq().await?;
            if let Some(received) = exec.collect(&mut frame).await? {
                return Ok::<_, RadioFault>(received);
            }
            // A preamble-only, header-damaged, or CRC-damaged event consumed
            // this IRQ but not the window. Keep the exact profile active until
            // a valid frame arrives or the declared dwell expires.
        }
    };
    let (result, received) = match with_timeout(
        Duration::from_millis(u64::from(receive.capture_dwell_ms)),
        capture,
    )
    .await
    {
        Ok(Ok(received)) => ("capture", Some(received)),
        Ok(Err(_)) => ("fault", None),
        Err(_) => ("miss", None),
    };
    let acquisition_us = acquisition_started.elapsed().as_micros();
    let captured = received.is_some();
    exec.note_scan_capture(id.0, captured);
    let restored = restore_profile(exec, previous).await;
    let (len, rssi, snr) = received
        .map(|received| (received.len, received.rssi, received.snr))
        .unwrap_or((0, 0, 0));
    let mut reply = radio_face::Text::<192>::empty();
    let _ = write!(
        &mut reply,
        "le3 rx id={} result={} len={} rssi={} snr={} apply={}us handoff={}us \
         acquisition={}us dwell={}ms restored={}\r\n",
        id.0,
        result,
        len,
        rssi,
        snr,
        arm.apply_us,
        arm.handoff_us,
        acquisition_us,
        receive.capture_dwell_ms,
        u8::from(restored),
    );
    if host.write_all(reply.as_str().as_bytes()).await.is_err() {
        Outcome::HostGone
    } else {
        Outcome::Served
    }
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
    // Whether the board can actually read LXMF, asked of the board rather than inferred
    // from the fact that it linked. Both halves are checked against captured stock answers
    // and report their own cost, because on a board the cost is half the question.
    //
    // Separate probes rather than one: stamp work takes seconds, and a host that reads to
    // the first newline would take the codec's answer and leave before the rest arrived.
    // One question, one line.
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
    // Live and peak allocation. The boot line is only a starting point; this preserves the
    // largest live allocation across a sustained replay flood after individual packet
    // buffers have been released.
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
