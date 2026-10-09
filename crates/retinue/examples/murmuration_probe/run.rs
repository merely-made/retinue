//! The bench sequence: session, baselines, excursions, deadline, and pin.

use sennet::flood::{
    FloodDecision, FloodIgnore, ManagedFlood, ManagedFloodConfig, RelayDelayWindow,
};
use sennet::node::Channel;
use sennet::transport::ChannelKey;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use tulle::personality::{
    ControllerError, ControllerEvent, Excursion, InterruptionPolicy, PauseOutcome, ReturnReason,
    StopCapability,
};
use tulle::personality_serial::PersonalitySerialError;

use crate::adapters::{RetinueAdapter, SennetAdapter};
use crate::excursion::{excursion, missed_wave, restore};
use crate::radio::{open, runtime, transfer};
use crate::{EXCURSION_MS, GAP, HOME, OTHER, Result, session};

pub(super) async fn run(
    dut_port: &str,
    peer_port: &str,
    output: &Path,
    events: &mut Vec<Value>,
) -> Result<()> {
    let mut random = [0u8; 32];
    getrandom::getrandom(&mut random).map_err(|e| format!("bench entropy: {e}"))?;
    let run_id = hex::encode(&random[16..24]);
    let key: [u8; 16] = random[..16].try_into()?;
    let source_dut = u32::from_le_bytes(random[24..28].try_into()?);
    let source_peer = u32::from_le_bytes(random[28..32].try_into()?);
    if source_dut == source_peer {
        return Err("bench source collision".into());
    }
    let channel = Channel {
        hash: 8,
        key: ChannelKey::Aes128(key),
    };
    let mut ret_dut = RetinueAdapter {
        prefix: format!("{run_id}-dut"),
        sequence: 0,
    };
    let mut ret_peer = RetinueAdapter {
        prefix: format!("{run_id}-peer"),
        sequence: 0,
    };
    let mut sen_dut = SennetAdapter::new(
        channel,
        source_dut,
        &output.with_file_name(format!("{run_id}-dut.sennet-state")),
    )?;
    let mut sen_peer = SennetAdapter::new(
        channel,
        source_peer,
        &output.with_file_name(format!("{run_id}-peer.sennet-state")),
    )?;
    let mut dut = runtime(open(dut_port, false).await?, false)?;
    let mut peer = runtime(open(peer_port, true).await?, false)?;
    let mut peer_seen = ManagedFlood::new(ManagedFloodConfig {
        channel_hash: channel.hash,
        relay_node: source_peer as u8,
        seen_capacity: 16,
        delay: RelayDelayWindow::new(Duration::ZERO, Duration::ZERO)?,
    })?;
    let mut first_sennet_frame: Option<Vec<u8>> = None;
    events.push(json!({"kind":"online","run_id":run_id,"dut":dut_port,"peer":peer_port,
        "frequency_hz":906875000,"bandwidth_hz":250000,"sf":11,"power_dbm":7,"home_sync":18,"away_sync":43}));
    let mut sessions = session::Sessions::establish(&mut dut, &mut peer, events).await?;
    sessions
        .exchange(&mut dut, &mut peer, "before-excursion", events)
        .await?;
    sessions
        .resource_before_excursion(&mut dut, &mut peer, events)
        .await?;
    sessions
        .forced_resource_interruption(&mut dut, &mut peer, events)
        .await?;
    transfer(
        &mut dut,
        &mut peer,
        HOME,
        ret_dut.frame("baseline"),
        None,
        "baseline-dut",
        events,
    )
    .await?;
    transfer(
        &mut peer,
        &mut dut,
        HOME,
        ret_peer.frame("baseline"),
        None,
        "baseline-peer",
        events,
    )
    .await?;
    // The seven-byte direct-PHY RX envelope must cross full USB packet boundaries.
    for length in [57, 121] {
        transfer(
            &mut peer,
            &mut dut,
            HOME,
            ret_peer.frame_with_length(length)?,
            None,
            &format!("usb-{}-peer", length + 7),
            events,
        )
        .await?;
        transfer(
            &mut dut,
            &mut peer,
            HOME,
            ret_dut.frame_with_length(length)?,
            None,
            &format!("usb-{}-dut", length + 7),
            events,
        )
        .await?;
    }
    for cycle in 0..2 {
        excursion(
            &mut dut,
            EXCURSION_MS,
            &format!("dut-{cycle}"),
            &sessions,
            events,
        )
        .await?;
        missed_wave(&mut dut, &mut peer, &mut ret_peer, cycle, events).await?;
        excursion(
            &mut peer,
            EXCURSION_MS,
            &format!("peer-{cycle}"),
            &sessions,
            events,
        )
        .await?;
        let a = sen_dut.frame(&format!("{run_id}:dut:{cycle}"))?;
        let b = sen_peer.frame(&format!("{run_id}:peer:{cycle}"))?;
        let received_a = transfer(
            &mut dut,
            &mut peer,
            OTHER,
            a.clone(),
            Some(channel),
            "sennet-dut",
            events,
        )
        .await?;
        if !matches!(
            peer_seen.consider(&received_a)?,
            FloodDecision::Ignore(FloodIgnore::HopLimit)
        ) {
            return Err("fresh Sennet frame was not new to retained dedup state".into());
        }
        if let Some(previous) = first_sennet_frame.as_ref() {
            let received_duplicate = transfer(
                &mut dut,
                &mut peer,
                OTHER,
                previous.clone(),
                Some(channel),
                "sennet-replay-after-pause",
                events,
            )
            .await?;
            if !matches!(
                peer_seen.consider(&received_duplicate)?,
                FloodDecision::Ignore(FloodIgnore::Duplicate)
            ) {
                return Err("Sennet duplicate state did not survive pause".into());
            }
            events.push(json!({"kind":"sennet_duplicate_retained","relay_decision":"Ignore(Duplicate)",
                "scope":"same RF frame recognized by retained ManagedFlood; not a session protocol"}));
        } else {
            first_sennet_frame = Some(a);
        }
        transfer(
            &mut peer,
            &mut dut,
            OTHER,
            b,
            Some(channel),
            "sennet-peer",
            events,
        )
        .await?;
        if cycle == 1 {
            let cancellation = dut.cancel()?;
            if cancellation != ControllerEvent::ReturnRequired(ReturnReason::Cancelled) {
                return Err(format!("unexpected cancellation: {cancellation:?}").into());
            }
            events
                .push(json!({"kind":"explicit_cancellation","event":format!("{cancellation:?}")}));
        }
        restore(
            &mut dut,
            &format!("dut-{cycle}"),
            cycle != 1,
            &sessions,
            events,
        )
        .await?;
        restore(&mut peer, &format!("peer-{cycle}"), true, &sessions, events).await?;
        sessions
            .exchange(
                &mut dut,
                &mut peer,
                &format!("after-excursion-{cycle}"),
                events,
            )
            .await?;
        transfer(
            &mut dut,
            &mut peer,
            HOME,
            ret_dut.frame("returned"),
            None,
            "returned-dut",
            events,
        )
        .await?;
        transfer(
            &mut peer,
            &mut dut,
            HOME,
            ret_peer.frame("returned"),
            None,
            "returned-peer",
            events,
        )
        .await?;
    }
    excursion(&mut dut, 1_000, "deadline-dut", &sessions, events).await?;
    match dut.recv().await {
        Err(PersonalitySerialError::ReceiveDeadline) => {}
        other => return Err(format!("deadline receive: {other:?}").into()),
    }
    let event = dut.tick(GAP)?;
    if event != ControllerEvent::ReturnRequired(ReturnReason::ExcursionExpired) {
        return Err(format!("deadline event: {event:?}").into());
    }
    restore(&mut dut, "deadline-dut", false, &sessions, events).await?;
    sessions
        .exchange(&mut dut, &mut peer, "after-deadline", events)
        .await?;
    let deadline_text =
        std::env::var("MC3_DEADLINE_TEXT").unwrap_or_else(|_| "after-deadline".into());
    transfer(
        &mut peer,
        &mut dut,
        HOME,
        ret_peer.frame(&deadline_text),
        None,
        "after-deadline",
        events,
    )
    .await?;
    // Rebuild immutable policy only at acknowledged home, retaining the SAME serial session.
    let mut dut = runtime(dut.release_at_home()?, true)?;
    let refusal = dut.request_excursion(
        Excursion {
            target: OTHER,
            duration_ms: 1_000,
            interruption: InterruptionPolicy::ResumableOnly,
        },
        GAP,
        PauseOutcome::Ready,
        StopCapability::Resumable,
    );
    if !matches!(
        refusal,
        Err(PersonalitySerialError::Controller(ControllerError::Pinned(
            HOME
        )))
    ) {
        return Err(format!("pin did not refuse: {refusal:?}").into());
    }
    events.push(
        json!({"kind":"pinned_refusal","result":format!("{refusal:?}"),"same_serial_session":true}),
    );
    transfer(
        &mut peer,
        &mut dut,
        HOME,
        ret_peer.frame("pinned-home"),
        None,
        "pinned-home",
        events,
    )
    .await?;
    events.push(json!({"kind":"retained_state","retinue_dut_sequence":ret_dut.sequence,
        "retinue_peer_sequence":ret_peer.sequence,"sennet_dut_reserved":sen_dut.sent,"sennet_peer_reserved":sen_peer.sent,
        "scope":"caller-owned packet counters and channel state; no remote sessions"}));
    sessions
        .exchange(&mut dut, &mut peer, "pinned-retained-link", events)
        .await?;
    dut.release_at_home()?.shutdown().await?;
    peer.release_at_home()?.shutdown().await?;
    Ok(())
}
