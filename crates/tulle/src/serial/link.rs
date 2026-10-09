//! The host handle to a running RNode serial pump.

use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use super::pump::{PumpChannels, TxRequest, run_pump};
use super::{PumpError, PumpStatus, SerialPumpConfig, TransmitError};
use crate::airtime::AirtimeBudget;
use crate::link::{RadioLink, Received};
use crate::lora::LoRaParams;
use crate::rnode::RNode;

/// A running RNode serial interface.
///
/// Opening asserts DTR, explicitly deasserts RTS, and starts one Tokio task that owns all
/// serial reads and writes. [`send`](Self::send) completes after the frame passes the airtime
/// gate and its KISS bytes have been written. [`recv`](Self::recv) yields complete RF frames.
pub struct RNodeSerialLink {
    tx: mpsc::Sender<TxRequest>,
    rx: mpsc::Receiver<Received>,
    errors: mpsc::Receiver<Vec<u8>>,
    status: watch::Receiver<PumpStatus>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), io::Error>>>,
}

impl RNodeSerialLink {
    /// Open a real serial port and start the pump.
    pub fn open(
        path: impl AsRef<Path>,
        params: LoRaParams,
        budget: AirtimeBudget,
        config: SerialPumpConfig,
    ) -> Result<Self, PumpError> {
        if config.tx_queue == 0 || config.rx_queue == 0 {
            return Err(PumpError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "serial pump queue capacities must be non-zero",
            )));
        }

        let port = serial2_tokio::SerialPort::open(path, config.baud_rate)?;
        // nRF USB CDC gates output on DTR. On ESP32, RTS is wired into reset/boot and must
        // remain deasserted (asserting it wedged a board in a live harness).
        port.set_dtr(true)?;
        port.set_rts(false)?;
        Ok(Self::spawn_io(port, params, budget, config))
    }

    pub(super) fn spawn_io<T>(
        io: T,
        params: LoRaParams,
        budget: AirtimeBudget,
        config: SerialPumpConfig,
    ) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, tx_rx) = mpsc::channel(config.tx_queue);
        let (rx_tx, rx) = mpsc::channel(config.rx_queue);
        let (status_tx, status) = watch::channel(PumpStatus::Settling);
        let (error_tx, errors) = mpsc::channel(config.rx_queue);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task_status = status_tx.clone();
        let task = tokio::spawn(async move {
            let result = run_pump(
                io,
                RadioLink::new(RNode::new(params), budget),
                config,
                PumpChannels {
                    tx_rx,
                    rx_tx,
                    status_tx,
                    error_tx,
                },
                shutdown_rx,
            )
            .await;
            match &result {
                Ok(()) => {
                    let _ = task_status.send(PumpStatus::Stopped);
                }
                Err(error) => {
                    let _ = task_status.send(PumpStatus::Fault(error.to_string()));
                }
            }
            result
        });

        Self {
            tx,
            rx,
            errors,
            status,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    /// Current lifecycle state without waiting.
    pub fn status(&self) -> PumpStatus {
        self.status.borrow().clone()
    }

    /// Wait until the RNode confirms that its radio is online.
    pub async fn wait_online(&mut self) -> Result<Option<(u8, u8)>, PumpError> {
        loop {
            match self.status.borrow().clone() {
                PumpStatus::Online { firmware } => return Ok(firmware),
                PumpStatus::Fault(message) => return Err(PumpError::Fault(message)),
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

impl Drop for RNodeSerialLink {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
