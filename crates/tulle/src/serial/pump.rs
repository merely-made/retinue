//! The pump task that owns the serial handle and the RNode state machine.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep, sleep_until};

use super::{PumpStatus, SerialPumpConfig, TransmitError};
use crate::link::{RadioLink, Received, SendOutcome};
use crate::modem::ModemError;
use crate::rnode::RNode;

pub(super) struct TxRequest {
    pub(super) frame: Vec<u8>,
    pub(super) announce: bool,
    pub(super) done: oneshot::Sender<Result<Duration, TransmitError>>,
}

/// The pump's ends of the channels it shares with its
/// [`RNodeSerialLink`](super::RNodeSerialLink).
pub(super) struct PumpChannels {
    pub(super) tx_rx: mpsc::Receiver<TxRequest>,
    pub(super) rx_tx: mpsc::Sender<Received>,
    pub(super) status_tx: watch::Sender<PumpStatus>,
    pub(super) error_tx: mpsc::Sender<Vec<u8>>,
}

pub(super) async fn run_pump<T>(
    mut io: T,
    mut link: RadioLink<RNode>,
    config: SerialPumpConfig,
    channels: PumpChannels,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(), io::Error>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let PumpChannels {
        mut tx_rx,
        rx_tx,
        status_tx,
        error_tx,
    } = channels;
    let epoch = Instant::now();
    if !config.open_settle.is_zero() {
        tokio::select! {
            _ = sleep(config.open_settle) => {}
            _ = &mut shutdown => return Ok(()),
        }
    }

    let _ = status_tx.send(PumpStatus::Initializing);
    link.modem_mut().start();
    flush_modem(&mut io, &mut link).await?;
    let mut next_init = Instant::now() + config.init_retry;
    let mut next_tx = Instant::now();
    let mut pending: Option<TxRequest> = None;
    let mut tx_closed = false;
    let mut announced_online = false;
    let mut read_buf = [0u8; 1024];

    loop {
        while let Some(received) = link.recv() {
            if rx_tx.send(received).await.is_err() {
                return Ok(());
            }
        }

        // The device's own complaints. Dropped rather than blocking the pump
        // if nobody is draining them: diagnostics must never stall traffic.
        if let Some(error) = link.modem_mut().take_last_error() {
            let _ = error_tx.try_send(error);
        }

        if link.modem().is_online() && !announced_online {
            announced_online = true;
            let _ = status_tx.send(PumpStatus::Online {
                firmware: link.modem().fw_version(),
            });
        }

        let now = Instant::now();
        if !link.modem().is_online() && now >= next_init {
            link.modem_mut().start();
            flush_modem(&mut io, &mut link).await?;
            next_init = Instant::now() + config.init_retry;
        }

        if pending.is_some() && link.modem().is_online() && now >= next_tx {
            let request = pending.take().expect("checked above");
            let now_ms = epoch.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            let outcome = if request.announce {
                link.send_announcement(&request.frame, now_ms)
            } else {
                link.send(&request.frame, now_ms)
            };
            match outcome {
                SendOutcome::Sent { airtime } => {
                    if let Err(error) = flush_modem(&mut io, &mut link).await {
                        let _ = request
                            .done
                            .send(Err(TransmitError::Transport(error.to_string())));
                        return Err(error);
                    }
                    next_tx = Instant::now() + airtime + config.turnaround;
                    let _ = request.done.send(Ok(airtime));
                }
                SendOutcome::DutyCycleBlocked {
                    retry_at_ms: Some(retry_at_ms),
                } => {
                    next_tx = epoch + Duration::from_millis(retry_at_ms);
                    pending = Some(request);
                }
                SendOutcome::DutyCycleBlocked { retry_at_ms: None } => {
                    let _ = request.done.send(Err(TransmitError::DutyCycleImpossible));
                }
                SendOutcome::AnnouncePaced {
                    retry_at_ms: Some(retry_at_ms),
                } => {
                    next_tx = epoch + Duration::from_millis(retry_at_ms);
                    pending = Some(request);
                }
                SendOutcome::AnnouncePaced { retry_at_ms: None } => {
                    let _ = request.done.send(Err(TransmitError::AnnouncementDisabled));
                }
                SendOutcome::Failed(ModemError::Busy) => {
                    next_tx = Instant::now() + config.busy_retry;
                    pending = Some(request);
                }
                SendOutcome::Failed(ModemError::TooLong { max }) => {
                    let _ = request.done.send(Err(TransmitError::TooLong { max }));
                }
                SendOutcome::Failed(ModemError::Unsupported) => {
                    let _ = request.done.send(Err(TransmitError::Unsupported));
                }
                SendOutcome::Failed(ModemError::Transport(error)) => {
                    let _ = request
                        .done
                        .send(Err(TransmitError::Transport(error.to_string())));
                }
            }
            continue;
        }

        let wake = if !link.modem().is_online() {
            next_init
        } else if pending.is_some() {
            next_tx
        } else {
            // A bounded wake keeps status/event draining prompt even on serial drivers that
            // do not produce a readiness edge for modem-control changes.
            Instant::now() + Duration::from_millis(250)
        };

        tokio::select! {
            biased;
            _ = &mut shutdown => return Ok(()),
            read = io.read(&mut read_buf) => {
                let count = read?;
                if count == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "serial port closed"));
                }
                link.modem_mut().on_serial(&read_buf[..count]);
                flush_modem(&mut io, &mut link).await?;
            }
            request = tx_rx.recv(), if pending.is_none() && !tx_closed => {
                match request {
                    Some(request) => pending = Some(request),
                    None => tx_closed = true,
                }
            }
            _ = sleep_until(wake) => {}
        }
    }
}

async fn flush_modem<T>(io: &mut T, link: &mut RadioLink<RNode>) -> Result<(), io::Error>
where
    T: AsyncWrite + Unpin,
{
    let bytes = link.modem_mut().take_outbound();
    if !bytes.is_empty() {
        io.write_all(&bytes).await?;
        io.flush().await?;
    }
    Ok(())
}
