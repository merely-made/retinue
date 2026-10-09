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
/// [`RNodeSerialLink`](super::RNodeSerialLink), kept across reopened ports.
pub(super) struct PumpChannels {
    pub(super) tx_rx: mpsc::Receiver<TxRequest>,
    pub(super) rx_tx: mpsc::Sender<Received>,
    pub(super) status_tx: watch::Sender<PumpStatus>,
    pub(super) error_tx: mpsc::Sender<Vec<u8>>,
    pub(super) shutdown: oneshot::Receiver<()>,
    /// A frame taken from `tx_rx` and not yet answered.
    pub(super) pending: Option<TxRequest>,
}

impl PumpChannels {
    /// Answer the frame in hand, if any, with `error`.
    pub(super) fn fail_pending(&mut self, error: TransmitError) {
        if let Some(request) = self.pending.take() {
            let _ = request.done.send(Err(error));
        }
    }
}

/// Run one open port. `Ok` means stop for good: shutdown, or nobody left to deliver to.
/// `Err` is a port or device failure that a supervisor may answer by reopening.
pub(super) async fn run_pump<T>(
    mut io: T,
    link: &mut RadioLink<RNode>,
    config: &SerialPumpConfig,
    ch: &mut PumpChannels,
) -> Result<(), io::Error>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let epoch = Instant::now();
    let _ = ch.status_tx.send(PumpStatus::Settling);
    if !config.open_settle.is_zero() {
        tokio::select! {
            _ = sleep(config.open_settle) => {}
            _ = &mut ch.shutdown => return Ok(()),
        }
    }

    let _ = ch.status_tx.send(PumpStatus::Initializing);
    link.modem_mut().start();
    flush_modem(&mut io, link).await?;
    let mut next_init = Instant::now() + config.init_retry;
    let mut next_tx = Instant::now();
    let mut locked_at = Instant::now();
    let mut last_read = Instant::now();
    let mut tx_closed = false;
    let mut announced_online = false;
    let mut read_buf = [0u8; 1024];

    loop {
        while let Some(received) = link.recv() {
            if ch.rx_tx.send(received).await.is_err() {
                return Ok(());
            }
        }

        // The device's own complaints. Dropped rather than blocking the pump
        // if nobody is draining them: diagnostics must never stall traffic.
        if let Some(error) = link.modem_mut().take_last_error() {
            let _ = ch.error_tx.try_send(error);
        }
        if let Some(fault) = link.modem_mut().take_fault() {
            return Err(io::Error::other(fault));
        }

        let online = link.modem().is_online();
        if online && !announced_online {
            announced_online = true;
            let _ = ch.status_tx.send(PumpStatus::Online {
                firmware: link.modem().fw_version(),
            });
        }

        let now = Instant::now();
        if !online && now >= next_init {
            link.modem_mut().start();
            flush_modem(&mut io, link).await?;
            next_init = Instant::now() + config.init_retry;
        }
        if link.modem().is_flow_locked() && now >= locked_at + config.flow_unlock {
            link.modem_mut().release();
        }

        if ch.pending.is_some() && online && now >= next_tx {
            let request = ch.pending.take().expect("checked above");
            let now_ms = epoch.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            let outcome = if request.announce {
                link.send_announcement(&request.frame, now_ms)
            } else {
                link.send(&request.frame, now_ms)
            };
            let answer = match outcome {
                SendOutcome::Sent { airtime } => {
                    if let Err(error) = flush_modem(&mut io, link).await {
                        let _ = request
                            .done
                            .send(Err(TransmitError::Transport(error.to_string())));
                        return Err(error);
                    }
                    locked_at = Instant::now();
                    next_tx = locked_at + airtime + config.turnaround;
                    Ok(airtime)
                }
                SendOutcome::DutyCycleBlocked {
                    retry_at_ms: Some(retry_at_ms),
                }
                | SendOutcome::AnnouncePaced {
                    retry_at_ms: Some(retry_at_ms),
                } => {
                    next_tx = epoch + Duration::from_millis(retry_at_ms);
                    ch.pending = Some(request);
                    continue;
                }
                SendOutcome::Failed(ModemError::Busy) => {
                    next_tx = Instant::now() + config.busy_retry;
                    ch.pending = Some(request);
                    continue;
                }
                SendOutcome::DutyCycleBlocked { retry_at_ms: None } => {
                    Err(TransmitError::DutyCycleImpossible)
                }
                SendOutcome::AnnouncePaced { retry_at_ms: None } => {
                    Err(TransmitError::AnnouncementDisabled)
                }
                SendOutcome::Failed(ModemError::TooLong { max }) => {
                    Err(TransmitError::TooLong { max })
                }
                SendOutcome::Failed(ModemError::Unsupported) => Err(TransmitError::Unsupported),
                SendOutcome::Failed(ModemError::Transport(error)) => {
                    Err(TransmitError::Transport(error.to_string()))
                }
            };
            let _ = request.done.send(answer);
            continue;
        }

        let wake = if !online {
            next_init
        } else if ch.pending.is_some() {
            next_tx
        } else {
            // A bounded wake keeps status/event draining prompt even on serial drivers that
            // do not produce a readiness edge for modem-control changes.
            Instant::now() + Duration::from_millis(250)
        };

        tokio::select! {
            biased;
            _ = &mut ch.shutdown => {
                // The detach handshake, best effort: the port may already be gone.
                link.modem_mut().leave();
                let _ = flush_modem(&mut io, link).await;
                return Ok(());
            }
            read = io.read(&mut read_buf) => {
                let count = read?;
                if count == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "serial port closed"));
                }
                if last_read.elapsed() > config.idle_reset {
                    link.modem_mut().reset_partial();
                }
                last_read = Instant::now();
                link.modem_mut().on_serial(&read_buf[..count]);
                flush_modem(&mut io, link).await?;
            }
            request = ch.tx_rx.recv(), if ch.pending.is_none() && !tx_closed => {
                match request {
                    Some(request) => ch.pending = Some(request),
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
