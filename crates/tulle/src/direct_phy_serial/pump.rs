//! The pump task that owns the serial handle and the direct-PHY state machine.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep, sleep_until};

use super::{DirectPhySerialConfig, ReconfigureError, UiSnapshotError, WakeSequence};
use crate::PhyProfile;
use crate::airtime::AirtimeBudget;
use crate::direct_phy::{self, Decoder, Event, MAX_FRAME_LEN};
use crate::link::Received;
use crate::lora::LoRaParams;
use crate::serial::{PumpStatus, TransmitError};

/// Charge a modeled duration conservatively in the millisecond budget domain.
///
/// The LoRa model retains sub-millisecond precision while [`AirtimeBudget`] is
/// deliberately millisecond based. Rounding down here would admit a stream
/// slightly faster than its configured duty or announce pacing cap.
pub(super) fn charge_duration_ms(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    let rounded = millis.saturating_add(u128::from(
        !duration.subsec_nanos().is_multiple_of(1_000_000),
    ));
    rounded.min(u128::from(u64::MAX)) as u64
}

const INITIALIZATION_RETRY: Duration = Duration::from_millis(500);

/// Write one host command, rousing the link first if this build needs it.
///
/// Every command goes through here so no path can forget the wake and leave a truncated frame
/// on a sleeping link.
async fn write_command<W: AsyncWrite + Unpin>(
    io: &mut W,
    wake: Option<&WakeSequence>,
    bytes: &[u8],
) -> io::Result<()> {
    if let Some(wake) = wake
        && !wake.preamble.is_empty()
    {
        io.write_all(&wake.preamble).await?;
        io.flush().await?;
        sleep(wake.settle).await;
    }
    io.write_all(bytes).await?;
    io.flush().await
}

pub(super) struct TxRequest {
    pub(super) frame: Vec<u8>,
    pub(super) announce: bool,
    pub(super) done: oneshot::Sender<Result<Duration, TransmitError>>,
}

pub(super) struct UiSnapshotRequest {
    pub(super) command: Vec<u8>,
    pub(super) done: oneshot::Sender<Result<(), UiSnapshotError>>,
}

pub(super) struct ProfileRequest {
    pub(super) command: Vec<u8>,
    pub(super) params: LoRaParams,
    pub(super) done: oneshot::Sender<Result<(), ReconfigureError>>,
}

struct InFlight {
    request: TxRequest,
    frame_len: usize,
    airtime: Duration,
    deadline: Instant,
}

struct UiSnapshotInFlight {
    done: oneshot::Sender<Result<(), UiSnapshotError>>,
    deadline: Instant,
}

struct ProfileInFlight {
    params: LoRaParams,
    done: oneshot::Sender<Result<(), ReconfigureError>>,
    deadline: Instant,
}

// The pump owns the whole state machine; bundling its inputs into a struct would
// only move the argument list one level out.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_pump<T>(
    mut io: T,
    profile: PhyProfile,
    mut params: LoRaParams,
    mut budget: AirtimeBudget,
    config: DirectPhySerialConfig,
    mut tx_rx: mpsc::Receiver<TxRequest>,
    mut profile_rx: mpsc::Receiver<ProfileRequest>,
    mut ui_snapshot_rx: mpsc::Receiver<UiSnapshotRequest>,
    rx_tx: mpsc::Sender<Received>,
    status_tx: watch::Sender<PumpStatus>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(), io::Error>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    if !config.open_settle.is_zero() {
        tokio::select! {
            _ = sleep(config.open_settle) => {}
            _ = &mut shutdown => return Ok(()),
        }
    }
    let _ = status_tx.send(PumpStatus::Initializing);
    let configure = direct_phy::encode_configure(profile).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid direct-PHY profile: {error:?}"),
        )
    })?;
    let mut decoder = Decoder::new();
    let mut read_buf = [0_u8; 1024];
    let mut status_bytes = Vec::new();
    let online_deadline = Instant::now() + config.online_timeout;
    let mut next_probe = Instant::now() + INITIALIZATION_RETRY;
    let mut saw_online = false;
    let mut configured = false;
    write_command(&mut io, config.wake.as_ref(), b"status\n").await?;
    sleep(Duration::from_millis(20)).await;
    write_command(&mut io, config.wake.as_ref(), &configure).await?;
    loop {
        let wake_at = online_deadline.min(next_probe);
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            _ = sleep_until(wake_at) => {
                if Instant::now() >= online_deadline {
                    let missing = match (saw_online, configured) {
                        (false, false) => "online status and radio profile acknowledgement",
                        (false, true) => "online status",
                        (true, false) => "radio profile acknowledgement",
                        (true, true) => unreachable!(),
                    };
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("direct-PHY firmware did not report {missing}"),
                    ));
                }
                write_command(&mut io, config.wake.as_ref(), b"status\n").await?;
                sleep(Duration::from_millis(20)).await;
                write_command(&mut io, config.wake.as_ref(), &configure).await?;
                next_probe = Instant::now() + INITIALIZATION_RETRY;
            }
            read = io.read(&mut read_buf) => {
                let count = read?;
                if count == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "direct-PHY serial port closed during initialization",
                    ));
                }
                status_bytes.extend_from_slice(&read_buf[..count]);
                if status_bytes.len() > 1024 {
                    status_bytes.drain(..status_bytes.len() - 1024);
                }
                let text = String::from_utf8_lossy(&status_bytes);
                if text.contains("tulle/") && text.contains("phy online") {
                    saw_online = true;
                }
                let mut events = Vec::new();
                decoder.push(&read_buf[..count], &mut events);
                for event in events {
                    match event {
                        Event::Configured { result: 0 } => configured = true,
                        Event::Configured { result } => {
                            return Err(io::Error::other(format!(
                                "direct-PHY firmware rejected the radio profile with result {result}"
                            )));
                        }
                        Event::UiSnapshot { .. } | Event::Observation(_) => {}
                        Event::Received(frame) => {
                            if rx_tx.send(frame).await.is_err() {
                                return Ok(());
                            }
                        }
                        Event::Transmitted { .. } | Event::Diagnostic { .. } => {}
                    }
                }
                if saw_online && configured {
                    break;
                }
            }
        }
    }
    let _ = status_tx.send(PumpStatus::Online { firmware: None });

    let epoch = Instant::now();
    let mut pending: Option<TxRequest> = None;
    let mut in_flight: Option<InFlight> = None;
    let mut retry_at: Option<Instant> = None;
    let mut tx_closed = false;
    let mut ui_snapshot_pending: Option<UiSnapshotRequest> = None;
    let mut ui_snapshot_in_flight: Option<UiSnapshotInFlight> = None;
    let mut ui_snapshot_closed = false;
    let mut profile_pending: Option<ProfileRequest> = None;
    let mut profile_in_flight: Option<ProfileInFlight> = None;
    let mut profile_closed = false;
    let mut resync_before_command = false;
    let mut last_diagnostic = None;

    loop {
        if profile_pending.is_none() && profile_in_flight.is_none() && !profile_closed {
            match profile_rx.try_recv() {
                Ok(request) => profile_pending = Some(request),
                Err(mpsc::error::TryRecvError::Disconnected) => profile_closed = true,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }

        if ui_snapshot_pending.is_none() && ui_snapshot_in_flight.is_none() && !ui_snapshot_closed {
            match ui_snapshot_rx.try_recv() {
                Ok(request) => ui_snapshot_pending = Some(request),
                Err(mpsc::error::TryRecvError::Disconnected) => ui_snapshot_closed = true,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }

        if pending.is_none()
            && in_flight.is_none()
            && profile_pending.is_none()
            && profile_in_flight.is_none()
            && ui_snapshot_pending.is_none()
            && ui_snapshot_in_flight.is_none()
            && !tx_closed
        {
            match tx_rx.try_recv() {
                Ok(request) => pending = Some(request),
                Err(mpsc::error::TryRecvError::Disconnected) => tx_closed = true,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }

        if pending.is_none()
            && in_flight.is_none()
            && profile_in_flight.is_none()
            && ui_snapshot_pending.is_none()
            && ui_snapshot_in_flight.is_none()
            && let Some(request) = profile_pending.take()
        {
            if resync_before_command {
                if config.wake.is_none() {
                    io.write_all(&[crate::WAKE_BYTE]).await?;
                    io.flush().await?;
                }
                resync_before_command = false;
            }
            write_command(&mut io, config.wake.as_ref(), &request.command).await?;
            profile_in_flight = Some(ProfileInFlight {
                params: request.params,
                done: request.done,
                deadline: Instant::now() + config.transmit_timeout,
            });
        }

        if profile_in_flight.is_none()
            && pending.is_none()
            && in_flight.is_none()
            && ui_snapshot_in_flight.is_none()
            && let Some(request) = ui_snapshot_pending.take()
        {
            if resync_before_command {
                if config.wake.is_none() {
                    io.write_all(&[crate::WAKE_BYTE]).await?;
                    io.flush().await?;
                }
                resync_before_command = false;
            }
            write_command(&mut io, config.wake.as_ref(), &request.command).await?;
            ui_snapshot_in_flight = Some(UiSnapshotInFlight {
                done: request.done,
                deadline: Instant::now() + config.transmit_timeout,
            });
        }

        if profile_pending.is_none()
            && profile_in_flight.is_none()
            && ui_snapshot_in_flight.is_none()
            && let Some(request) = pending.take()
        {
            if request.frame.len() > MAX_FRAME_LEN {
                let _ = request
                    .done
                    .send(Err(TransmitError::TooLong { max: MAX_FRAME_LEN }));
                retry_at = None;
                continue;
            }
            let now_ms = epoch.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            let airtime_ms = charge_duration_ms(params.time_on_air(request.frame.len()));
            let may_transmit = if request.announce {
                budget.may_transmit_announce(now_ms, airtime_ms)
            } else {
                budget.may_transmit(now_ms, airtime_ms)
            };
            if may_transmit {
                let command = direct_phy::encode_transmit(&request.frame)
                    .expect("frame length checked above");
                if resync_before_command {
                    if config.wake.is_none() {
                        io.write_all(&[crate::WAKE_BYTE]).await?;
                        io.flush().await?;
                    }
                    resync_before_command = false;
                }
                write_command(&mut io, config.wake.as_ref(), &command).await?;
                if request.announce {
                    budget.record_announce(now_ms, airtime_ms);
                } else {
                    budget.record(now_ms, airtime_ms);
                }
                let airtime = params.time_on_air(request.frame.len());
                in_flight = Some(InFlight {
                    frame_len: request.frame.len(),
                    request,
                    airtime,
                    deadline: Instant::now() + config.transmit_timeout,
                });
                retry_at = None;
            } else if let Some(next_ms) = if request.announce {
                budget.next_announce_slot(now_ms, airtime_ms)
            } else {
                budget.next_slot(now_ms, airtime_ms)
            } {
                pending = Some(request);
                retry_at = Some(epoch + Duration::from_millis(next_ms));
            } else {
                let error = if request.announce
                    && budget
                        .announce_pacing()
                        .cooldown_after_send_ms(airtime_ms)
                        .is_none()
                {
                    TransmitError::AnnouncementDisabled
                } else {
                    TransmitError::DutyCycleImpossible
                };
                let _ = request.done.send(Err(error));
                retry_at = None;
            }
        }

        if tx_closed
            && profile_closed
            && ui_snapshot_closed
            && pending.is_none()
            && in_flight.is_none()
            && profile_pending.is_none()
            && profile_in_flight.is_none()
            && ui_snapshot_pending.is_none()
            && ui_snapshot_in_flight.is_none()
        {
            return Ok(());
        }

        let mut wake_at = match (&in_flight, retry_at) {
            (Some(sent), Some(retry)) => sent.deadline.min(retry),
            (Some(sent), None) => sent.deadline,
            (None, Some(retry)) => retry,
            (None, None) => Instant::now() + Duration::from_secs(3600),
        };
        if let Some(snapshot) = &ui_snapshot_in_flight {
            wake_at = wake_at.min(snapshot.deadline);
        }
        if let Some(profile) = &profile_in_flight {
            wake_at = wake_at.min(profile.deadline);
        }

        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            request = tx_rx.recv(), if pending.is_none()
                && in_flight.is_none()
                && profile_pending.is_none()
                && profile_in_flight.is_none()
                && ui_snapshot_pending.is_none()
                && ui_snapshot_in_flight.is_none()
                && !tx_closed => {
                match request {
                    Some(request) => pending = Some(request),
                    None => tx_closed = true,
                }
            }
            request = ui_snapshot_rx.recv(), if ui_snapshot_pending.is_none() && ui_snapshot_in_flight.is_none() && !ui_snapshot_closed => {
                match request {
                    Some(request) => ui_snapshot_pending = Some(request),
                    None => ui_snapshot_closed = true,
                }
            }
            request = profile_rx.recv(), if profile_pending.is_none() && profile_in_flight.is_none() && !profile_closed => {
                match request {
                    Some(request) => profile_pending = Some(request),
                    None => profile_closed = true,
                }
            }
            read = io.read(&mut read_buf) => {
                let count = read?;
                if count == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "direct-PHY serial port closed"));
                }
                let mut events = Vec::new();
                decoder.push(&read_buf[..count], &mut events);
                for event in events {
                    match event {
                        Event::Received(frame) => {
                            if rx_tx.send(frame).await.is_err() {
                                return Ok(());
                            }
                        }
                        Event::Transmitted { result, frame_len } => {
                            if let Some(sent) = in_flight.take() {
                                let outcome = if result == 0 && frame_len == sent.frame_len {
                                    Ok(sent.airtime)
                                } else {
                                    let diagnostic = last_diagnostic
                                        .take()
                                        .map(|(irq, errors, sync): (u16, u16, [u8; 2])| {
                                            format!(
                                                "; irq=0x{irq:04x} errors=0x{errors:04x} sync={:02x}{:02x}",
                                                sync[0], sync[1]
                                            )
                                        })
                                        .unwrap_or_default();
                                    Err(TransmitError::Transport(format!(
                                        "direct-PHY transmit result {result}, length {frame_len}, expected {}{diagnostic}",
                                        sent.frame_len,
                                    )))
                                };
                                let _ = sent.request.done.send(outcome);
                            }
                        }
                        Event::Configured { result } => {
                            if let Some(request) = profile_in_flight.take() {
                                let outcome = if result == selvage::CONFIG_ACCEPTED {
                                    params = request.params;
                                    Ok(())
                                } else {
                                    Err(ReconfigureError::Rejected { result })
                                };
                                let _ = request.done.send(outcome);
                            }
                        }
                        Event::UiSnapshot { result } => {
                            if let Some(snapshot) = ui_snapshot_in_flight.take() {
                                let outcome = if result == selvage::UI_SNAPSHOT_ACCEPTED {
                                    Ok(())
                                } else {
                                    Err(UiSnapshotError::Rejected { result })
                                };
                                let _ = snapshot.done.send(outcome);
                            }
                        }
                        Event::Observation(_) => {}
                        Event::Diagnostic {
                            irq_status,
                            device_errors,
                            sync_word,
                        } => {
                            last_diagnostic = Some((irq_status, device_errors, sync_word));
                        }
                    }
                }
            }
            _ = sleep_until(wake_at) => {
                if in_flight.as_ref().is_some_and(|sent| sent.deadline <= Instant::now())
                    && let Some(sent) = in_flight.take()
                {
                    let _ = sent.request.done.send(Err(TransmitError::Transport(
                        "direct-PHY transmit acknowledgement timed out".to_string(),
                    )));
                }
                if retry_at.is_some_and(|retry| retry <= Instant::now()) {
                    retry_at = None;
                }
                if ui_snapshot_in_flight
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.deadline <= Instant::now())
                    && let Some(snapshot) = ui_snapshot_in_flight.take()
                {
                    let _ = snapshot.done.send(Err(UiSnapshotError::TimedOut));
                    resync_before_command = true;
                }
                if profile_in_flight
                    .as_ref()
                    .is_some_and(|profile| profile.deadline <= Instant::now())
                    && let Some(profile) = profile_in_flight.take()
                {
                    let _ = profile.done.send(Err(ReconfigureError::TimedOut));
                    resync_before_command = true;
                }
            }
        }
    }
}
