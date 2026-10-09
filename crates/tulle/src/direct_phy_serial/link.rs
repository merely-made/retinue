//! The host handle to a running direct-PHY serial pump.

use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use super::pump::{ProfileRequest, TxRequest, UiSnapshotRequest, run_pump};
use super::{DirectPhySerialConfig, ReconfigureError, UiSnapshotError};
use crate::PhyProfile;
use crate::airtime::AirtimeBudget;
use crate::direct_phy;
use crate::link::Received;
use crate::lora::LoRaParams;
use crate::serial::{PumpError, PumpStatus, TransmitError};

/// Cloneable control lane for publishing host projections while a packet
/// driver owns the radio link.
///
/// This handle carries opaque snapshot bytes only. It cannot transmit or
/// receive radio frames, inspect the display schema, or shut the link down.
#[derive(Clone)]
pub struct DirectPhyUiControl {
    ui_snapshot: mpsc::Sender<UiSnapshotRequest>,
}

impl DirectPhyUiControl {
    pub async fn publish(&self, snapshot: &[u8]) -> Result<(), UiSnapshotError> {
        let command = direct_phy::encode_ui_snapshot(snapshot).map_err(|error| match error {
            direct_phy::EncodeError::UiSnapshotTooLong { actual } => {
                UiSnapshotError::TooLong { actual }
            }
            direct_phy::EncodeError::TooLong { actual } => UiSnapshotError::TooLong { actual },
        })?;
        let (done, result) = oneshot::channel();
        self.ui_snapshot
            .send(UiSnapshotRequest { command, done })
            .await
            .map_err(|_| UiSnapshotError::Stopped)?;
        result.await.unwrap_or(Err(UiSnapshotError::Stopped))
    }
}

/// A running serial connection to Tulle direct-PHY firmware.
pub struct DirectPhySerialLink {
    tx: mpsc::Sender<TxRequest>,
    profile: mpsc::Sender<ProfileRequest>,
    ui_snapshot: mpsc::Sender<UiSnapshotRequest>,
    rx: mpsc::Receiver<Received>,
    status: watch::Receiver<PumpStatus>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), io::Error>>>,
}

impl DirectPhySerialLink {
    pub fn open(
        path: impl AsRef<Path>,
        profile: PhyProfile,
        budget: AirtimeBudget,
        config: DirectPhySerialConfig,
    ) -> Result<Self, PumpError> {
        if config.tx_queue == 0 || config.rx_queue == 0 {
            return Err(PumpError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "direct-PHY queue capacities must be non-zero",
            )));
        }
        let port = serial2_tokio::SerialPort::open(path, config.baud_rate)?;
        port.set_dtr(config.dtr)?;
        port.set_rts(config.rts)?;
        let params = LoRaParams::try_from(profile).map_err(|message| {
            PumpError::Io(io::Error::new(io::ErrorKind::InvalidInput, message))
        })?;
        Ok(Self::spawn_io(port, profile, params, budget, config))
    }

    /// Queue an ordinary packet through the shared airtime gate.
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

    /// Apply a new complete radio profile without dropping the serial session.
    ///
    /// Some native-USB firmware cannot observe a host detach and emits its online
    /// event only once per boot. Reconfiguration therefore belongs inside the
    /// running pump, serialized against transmit and UI commands.
    pub async fn reconfigure(&self, profile: PhyProfile) -> Result<(), ReconfigureError> {
        let command = direct_phy::encode_configure(profile)
            .map_err(|error| ReconfigureError::InvalidProfile(format!("{error:?}")))?;
        let params = LoRaParams::try_from(profile)
            .map_err(|message| ReconfigureError::InvalidProfile(message.to_string()))?;
        let (done, result) = oneshot::channel();
        self.profile
            .send(ProfileRequest {
                command: command.to_vec(),
                params,
                done,
            })
            .await
            .map_err(|_| ReconfigureError::Stopped)?;
        result.await.unwrap_or(Err(ReconfigureError::Stopped))
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

    pub(super) fn spawn_io<T>(
        io: T,
        profile: PhyProfile,
        params: LoRaParams,
        budget: AirtimeBudget,
        config: DirectPhySerialConfig,
    ) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, tx_rx) = mpsc::channel(config.tx_queue);
        let (profile_tx, profile_rx) = mpsc::channel(1);
        let (ui_snapshot, ui_snapshot_rx) = mpsc::channel(1);
        let (rx_tx, rx) = mpsc::channel(config.rx_queue);
        let (status_tx, status) = watch::channel(PumpStatus::Settling);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task_status = status_tx.clone();
        let task = tokio::spawn(async move {
            let result = run_pump(
                io,
                profile,
                params,
                budget,
                config,
                tx_rx,
                profile_rx,
                ui_snapshot_rx,
                rx_tx,
                status_tx,
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
            profile: profile_tx,
            ui_snapshot,
            rx,
            status,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_test_io<T>(
        io: T,
        profile: PhyProfile,
        budget: AirtimeBudget,
        config: DirectPhySerialConfig,
    ) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let params = LoRaParams::try_from(profile).expect("test profile must be valid");
        Self::spawn_io(io, profile, params, budget, config)
    }

    pub fn status(&self) -> PumpStatus {
        self.status.borrow().clone()
    }

    pub async fn wait_online(&mut self) -> Result<(), PumpError> {
        loop {
            match self.status.borrow().clone() {
                PumpStatus::Online { .. } => return Ok(()),
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

    /// Obtain the UI-only control lane before moving this link into a packet
    /// driver.
    pub fn ui_control(&self) -> DirectPhyUiControl {
        DirectPhyUiControl {
            ui_snapshot: self.ui_snapshot.clone(),
        }
    }

    /// Publish an opaque, versioned host projection to the on-device UI.
    ///
    /// Tulle owns framing and acknowledgement only. The payload schema remains
    /// `radio-face`'s responsibility at the host and firmware edges.
    pub async fn publish_ui_snapshot(&self, snapshot: &[u8]) -> Result<(), UiSnapshotError> {
        self.ui_control().publish(snapshot).await
    }

    pub async fn recv(&mut self) -> Option<Received> {
        self.rx.recv().await
    }

    /// Discards frames already delivered by the serial pump but not yet assigned
    /// to a protocol adapter. The personality runtime uses this at a profile
    /// boundary so an old-profile frame is never delivered to the new adapter.
    pub(crate) fn discard_buffered_rx(&mut self) -> usize {
        let mut discarded = 0;
        while self.rx.try_recv().is_ok() {
            discarded += 1;
        }
        discarded
    }

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

impl Drop for DirectPhySerialLink {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
