//! The host handle to a running RNode serial pump.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::pump::{PumpChannels, TxRequest, run_pump};
use super::{PumpError, PumpStatus, SerialPumpConfig, TransmitError};
use crate::airtime::AirtimeBudget;
use crate::link::{RadioLink, Received};
use crate::lora::LoRaParams;
use crate::rnode::{RNode, RNodeConfig};

/// A running RNode serial interface.
///
/// Opening asserts DTR and explicitly deasserts RTS, best effort, and starts one Tokio task
/// that owns all serial reads and writes. [`send`](Self::send) completes after the frame
/// passes the airtime gate and its KISS bytes have been written. [`recv`](Self::recv) yields
/// complete RF frames. Shutting down, or dropping the handle, turns the radio off and sends
/// LEAVE before the port closes.
pub struct RNodeSerialLink {
    tx: mpsc::Sender<TxRequest>,
    rx: mpsc::Receiver<Received>,
    errors: mpsc::Receiver<Vec<u8>>,
    pub(super) status: watch::Receiver<PumpStatus>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), io::Error>>>,
    supervised: bool,
}

impl RNodeSerialLink {
    /// Open a real serial port and start the pump.
    pub fn open(
        path: impl AsRef<Path>,
        params: LoRaParams,
        budget: AirtimeBudget,
        config: SerialPumpConfig,
    ) -> Result<Self, PumpError> {
        Self::open_with(path, RNodeConfig::new(params), budget, config)
    }

    /// Open a real serial port with a full RNode configuration. The pump ends on the first
    /// port or device failure.
    pub fn open_with(
        path: impl AsRef<Path>,
        rnode: RNodeConfig,
        budget: AirtimeBudget,
        config: SerialPumpConfig,
    ) -> Result<Self, PumpError> {
        check(&rnode, &config)?;
        let mut port = Some(super::port::open(path.as_ref(), config.baud_rate)?);
        let open = move || port.take().ok_or_else(|| io::ErrorKind::NotFound.into());
        Ok(Self::spawn(open, rnode, budget, config, false))
    }

    /// Run an RNode that is reopened [`SerialPumpConfig::reconnect`] after any port or device
    /// failure, as RNS reconnects (`RNodeInterface.py` 1175-1187). Frames offered while it is
    /// down fail with [`TransmitError::Offline`].
    pub fn supervise(
        path: impl Into<PathBuf>,
        rnode: RNodeConfig,
        budget: AirtimeBudget,
        config: SerialPumpConfig,
    ) -> Result<Self, PumpError> {
        check(&rnode, &config)?;
        let path = path.into();
        let baud = config.baud_rate;
        Ok(Self::spawn(
            move || super::port::open(&path, baud),
            rnode,
            budget,
            config,
            true,
        ))
    }

    #[cfg(test)]
    pub(super) fn spawn_io<T>(
        io: T,
        params: LoRaParams,
        budget: AirtimeBudget,
        config: SerialPumpConfig,
    ) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let mut io = Some(io);
        let open = move || io.take().ok_or_else(|| io::ErrorKind::NotFound.into());
        Self::spawn(open, RNodeConfig::new(params), budget, config, false)
    }

    pub(super) fn spawn<T, F>(
        mut open: F,
        rnode: RNodeConfig,
        budget: AirtimeBudget,
        config: SerialPumpConfig,
        supervised: bool,
    ) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        F: FnMut() -> io::Result<T> + Send + 'static,
    {
        let (tx, tx_rx) = mpsc::channel(config.tx_queue);
        let (rx_tx, rx) = mpsc::channel(config.rx_queue);
        let (status_tx, status) = watch::channel(PumpStatus::Settling);
        let (error_tx, errors) = mpsc::channel(config.rx_queue);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let mut ch = PumpChannels {
            tx_rx,
            rx_tx,
            status_tx,
            error_tx,
            shutdown: shutdown_rx,
            pending: None,
        };
        let task = tokio::spawn(async move {
            let mut link = RadioLink::new(RNode::with_config(rnode), budget);
            loop {
                let result = match open() {
                    Ok(io) => run_pump(io, &mut link, &config, &mut ch).await,
                    Err(error) => Err(error),
                };
                let error = match result {
                    Ok(()) => break,
                    Err(error) => error,
                };
                let _ = ch.status_tx.send(PumpStatus::Fault(error.to_string()));
                if !supervised {
                    return Err(error);
                }
                ch.fail_pending(TransmitError::Offline);
                if !wait_offline(config.reconnect, &mut ch).await {
                    break;
                }
                link.modem_mut().reopen();
            }
            let _ = ch.status_tx.send(PumpStatus::Stopped);
            Ok(())
        });

        Self {
            tx,
            rx,
            errors,
            status,
            shutdown: Some(shutdown),
            task: Some(task),
            supervised,
        }
    }

    /// A receiver of every lifecycle change, for a caller that hands the link to a driver.
    pub fn watch_status(&self) -> watch::Receiver<PumpStatus> {
        self.status.clone()
    }

    /// Current lifecycle state without waiting.
    pub fn status(&self) -> PumpStatus {
        self.status.borrow().clone()
    }

    /// Wait until the RNode confirms that its radio is online. A supervised link keeps
    /// waiting through faults, since it reopens.
    pub async fn wait_online(&mut self) -> Result<Option<(u8, u8)>, PumpError> {
        loop {
            match self.status.borrow().clone() {
                PumpStatus::Online { firmware } => return Ok(firmware),
                PumpStatus::Fault(message) if !self.supervised => {
                    return Err(PumpError::Fault(message));
                }
                PumpStatus::Fault(_) => {}
                PumpStatus::Stopped => return Err(PumpError::Stopped),
                PumpStatus::Settling | PumpStatus::Initializing => {}
            }
            self.status
                .changed()
                .await
                .map_err(|_| PumpError::Stopped)?;
        }
    }

    /// Queue one complete RF frame and wait until it has passed the shared airtime gate and
    /// its serial bytes have been written to the RNode.
    pub async fn send(&self, frame: impl Into<Vec<u8>>) -> Result<Duration, TransmitError> {
        self.queue(frame.into(), false).await
    }

    /// Queue a Reticulum announce through the same duty gate plus this interface's
    /// announce-specific pacing cap.
    pub async fn send_announcement(
        &self,
        frame: impl Into<Vec<u8>>,
    ) -> Result<Duration, TransmitError> {
        self.queue(frame.into(), true).await
    }

    async fn queue(&self, frame: Vec<u8>, announce: bool) -> Result<Duration, TransmitError> {
        let (done, result) = oneshot::channel();
        self.tx
            .send(TxRequest {
                frame,
                announce,
                done,
            })
            .await
            .map_err(|_| TransmitError::Stopped)?;
        result.await.unwrap_or(Err(TransmitError::Stopped))
    }

    /// Receive the next complete RF frame and its link metrics.
    pub async fn recv(&mut self) -> Option<Received> {
        self.rx.recv().await
    }

    /// Take the next `ERROR` frame the device reported, if any is waiting.
    ///
    /// Non-blocking, so a caller can check it beside ordinary traffic. The device
    /// latches these when it refuses something; unread, a radio that silently
    /// declines to transmit looks healthy from the host
    /// (`design_docs/2026-07-26_rnode_bulk_frame_loss.md`).
    pub fn take_device_error(&mut self) -> Option<Vec<u8>> {
        self.errors.try_recv().ok()
    }

    /// Stop the task and wait for the serial port to close.
    pub async fn shutdown(mut self) -> Result<(), PumpError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        match self.task.take() {
            Some(task) => task.await.map_err(PumpError::Task)?.map_err(PumpError::Io),
            None => Ok(()),
        }
    }
}

/// Signals the task rather than aborting it, so it can still send the detach handshake.
impl Drop for RNodeSerialLink {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

fn check(rnode: &RNodeConfig, config: &SerialPumpConfig) -> Result<(), PumpError> {
    let invalid =
        |message: String| PumpError::Io(io::Error::new(io::ErrorKind::InvalidInput, message));
    if config.tx_queue == 0 || config.rx_queue == 0 {
        return Err(invalid(
            "serial pump queue capacities must be non-zero".into(),
        ));
    }
    rnode.validate().map_err(|error| invalid(error.to_string()))
}

/// Fail offered frames until `delay` passes. False if the link was shut down meanwhile.
async fn wait_offline(delay: Duration, ch: &mut PumpChannels) -> bool {
    let reopen = sleep(delay);
    tokio::pin!(reopen);
    loop {
        tokio::select! {
            _ = &mut reopen => return true,
            _ = &mut ch.shutdown => return false,
            request = ch.tx_rx.recv() => match request {
                Some(request) => {
                    let _ = request.done.send(Err(TransmitError::Offline));
                }
                None => return false,
            },
        }
    }
}
