//! Excursions away from HOME, the return, and the missed HOME wave.

use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::{Instant, timeout};
use tulle::personality::{ControllerState, Excursion, InterruptionPolicy, StopCapability};
use tulle::personality_serial::PersonalitySerialRuntime;

use crate::adapters::RetinueAdapter;
use crate::{GAP, HOME, OTHER, Result, session};

pub(super) async fn excursion(
    r: &mut PersonalitySerialRuntime,
    ms: u64,
    label: &str,
    sessions: &session::Sessions,
    events: &mut Vec<Value>,
) -> Result<()> {
    let readiness = sessions.assess_pause(
        ms.checked_add(11_000).ok_or("pause horizon overflow")?,
        events,
    )?;
    let t = r
        .request_excursion(
            Excursion {
                target: OTHER,
                duration_ms: ms,
                interruption: InterruptionPolicy::ResumableOnly,
            },
            GAP,
            readiness,
            StopCapability::Resumable,
        )?
        .ok_or("unexpected busy adapter")?;
    let start = Instant::now();
    let ack = r.apply_transition(t).await?;
    if !matches!(
        r.controller().state(),
        ControllerState::Away { target: OTHER, .. }
    ) {
        return Err("activation not acknowledged".into());
    }
    events.push(json!({"kind":"excursion_ack","label":label,"transition_id":t.id,
        "elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"discarded_rx":ack.discarded_rx,"state":format!("{:?}",r.controller().state())}));
    Ok(())
}
pub(super) async fn restore(
    r: &mut PersonalitySerialRuntime,
    label: &str,
    finish: bool,
    sessions: &session::Sessions,
    events: &mut Vec<Value>,
) -> Result<()> {
    // Six seconds conservatively covers the runtime's five-second return budget.
    let readiness = sessions.assess_pause(6_000, events)?;
    let start = Instant::now();
    if finish {
        r.finish()?;
    }
    let t = r
        .begin_return(readiness)?
        .ok_or("unexpected return deferral")?;
    let ack = r.apply_transition(t).await?;
    if r.controller().state() != ControllerState::Home {
        return Err("home not acknowledged".into());
    }
    events.push(json!({"kind":"home_ack","label":label,"transition_id":t.id,
        "return_ms":start.elapsed().as_secs_f64()*1000.0,"discarded_rx":ack.discarded_rx}));
    let settle_ms: u64 = std::env::var("MC3_SETTLE_MS")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    if settle_ms > 500 {
        return Err("bench settle interval exceeds 500ms".into());
    }
    if settle_ms != 0 {
        tokio::time::sleep(Duration::from_millis(settle_ms)).await;
        events.push(
            json!({"kind":"post_ack_settle","label":label,"requested_ms":settle_ms,
            "return_with_settle_ms":start.elapsed().as_secs_f64()*1000.0}),
        );
    }
    Ok(())
}
pub(super) async fn missed_wave(
    dut: &mut PersonalitySerialRuntime,
    peer: &mut PersonalitySerialRuntime,
    adapter: &mut RetinueAdapter,
    cycle: u32,
    events: &mut Vec<Value>,
) -> Result<()> {
    let start = Instant::now();
    let mut expected = Vec::new();
    for n in 0..2 {
        let frame = adapter.frame(&format!("absent-home-{cycle}-{n}"));
        let air = peer.send(HOME, frame.clone()).await?;
        events.push(json!({"kind":"home_wave_tx_ack","cycle":cycle,"hex":hex::encode(&frame),"airtime_ms":air.as_secs_f64()*1000.0}));
        expected.push(frame);
    }
    let until = Instant::now() + Duration::from_millis(900);
    let mut seen = Vec::new();
    let mut unrelated = 0;
    while Instant::now() < until {
        match timeout(until.saturating_duration_since(Instant::now()), dut.recv()).await {
            Ok(Ok(Some(frame))) => {
                if expected.contains(&frame.frame) {
                    seen.push(frame.frame);
                } else {
                    unrelated += 1;
                }
            }
            Ok(Ok(None)) => return Err("DUT stopped during absence measurement".into()),
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => break,
        }
    }
    let captured = expected.iter().filter(|e| seen.contains(e)).count();
    events.push(json!({"kind":"home_absence","cycle":cycle,"injected_tx_acked":2,"captured":captured,
        "not_observed":2-captured,"unrelated_frames":unrelated,"interval_ms":start.elapsed().as_secs_f64()*1000.0,
        "scope":"DUT receive opportunity absent; no independent on-air witness"}));
    if captured != 0 {
        return Err("DUT unexpectedly captured home profile while away".into());
    }
    Ok(())
}
