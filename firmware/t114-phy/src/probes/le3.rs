//! LE3 scan-plan probes: report the plan, and run CAD or capture trials on one profile.

use core::fmt::Write as _;

use embassy_time::{Duration, Instant, Timer, with_timeout};
use radio_hand::executive::{Executive, RadioFault};
use radio_hand::link::HostLink;
use radio_hand::profiles::{DetectionProfileId, ReceiveProfileId};

use super::Outcome;
use crate::le3;

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

pub(super) async fn serve_le3_plan<L, RK, DLY>(
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

pub(super) async fn serve_le3_cad<L, RK, DLY>(
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

pub(super) async fn serve_le3_rx<L, RK, DLY>(
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
    // Lets the ready marker reach the host. RX is already armed, so an early frame stays
    // latched by DIO1.
    Timer::after_millis(50).await;
    let acquisition_started = Instant::now();
    let mut frame = [0_u8; 255];
    let capture = async {
        loop {
            exec.wait_rx_irq().await?;
            if let Some(received) = exec.collect(&mut frame).await? {
                return Ok::<_, RadioFault>(received);
            }
            // A damaged event consumed this IRQ, not the window: keep listening until a
            // valid frame or the dwell expires.
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
