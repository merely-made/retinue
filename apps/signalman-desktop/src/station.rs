//! The live station actor.
//!
//! One ordinary worker thread owns a small Tokio runtime and the lease-gated
//! `mere_signalman::SitedStation`. It polls the port's read-only management
//! snapshot getter and hands owned Postilion snapshots to the UI thread, which
//! projects them under the owner's stale policy. The actor never touches
//! Retinue routing state directly, renders nothing, and holds no policy: a
//! closed lease or failed station simply becomes a terminal event.
//!
//! The station host wallet, delegated station identity, and durable device id
//! live under an owner-local data root. Reopening the application therefore
//! reopens the same station address, matching the sealed-station contract.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mere_signalman::{SitedStation, SitedStationCredential, SitedStationError};
use pandect::{DeviceId, ensure_wallet_state};
use personae::{InMemoryProvider, PersonaId};
use postilion::management::ManagementSnapshot;

use crate::network::LayoutWake;

/// How often the actor captures a snapshot while the station is live.
const POLL: Duration = Duration::from_secs(2);
/// Each self-issued host grant covers this window.
const GRANT_WINDOW: Duration = Duration::from_secs(15 * 60);
/// A replacement grant is issued when less than this much window remains, so
/// renewal always lands before the lease deadline.
const RENEW_MARGIN: Duration = Duration::from_secs(5 * 60);

/// Everything the actor needs to open one attended local station.
#[derive(Clone, Debug)]
pub struct StationSettings {
    /// Wallet root, delegated identity, and device-id record home.
    pub data_root: PathBuf,
    /// The serial port carrying a running Retinue board.
    pub port: String,
    /// The announced station name.
    pub name: String,
}

/// Read the live-station activation from the environment.
///
/// The live path stays off unless `SIGNALMAN_STATION_PORT` names a port. This
/// mirrors the other runtime-shaping variables (`SIGNALMAN_SERIAL_PORTS`,
/// `SIGNALMAN_MESSAGE_STORE`): bench activation without inventing UI policy
/// the management plan has not gated yet.
pub fn settings_from_env() -> Option<StationSettings> {
    let port = std::env::var("SIGNALMAN_STATION_PORT").ok()?;
    if port.trim().is_empty() {
        return None;
    }
    let data_root = std::env::var_os("SIGNALMAN_STATION_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let root = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            root.join("Merely").join("Signalman").join("station")
        });
    let name = std::env::var("SIGNALMAN_STATION_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Signalman station".to_owned());
    Some(StationSettings {
        data_root,
        port,
        name,
    })
}

/// What the actor observed, in order. Snapshots are facts for projection;
/// the other variants are presentation status only.
pub enum StationEvent {
    /// The lease-gated station opened.
    Connected {
        name: String,
        port: String,
        expires_at_ms: u64,
    },
    /// One owned management capture and its wall-clock capture time.
    Snapshot {
        snapshot: Box<ManagementSnapshot>,
        captured_unix_ms: u64,
    },
    /// The actor stopped and will not produce further snapshots.
    Failed { message: String },
}

/// One live station owned by one ordinary worker thread.
pub struct StationWorker {
    events: Receiver<StationEvent>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl StationWorker {
    pub fn spawn(settings: StationSettings, wake: LayoutWake) -> Self {
        let (events_tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let actor_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("signalman-station".to_owned())
            .spawn(move || run_station(settings, events_tx, wake, actor_stop))
            .expect("spawn Signalman station actor");
        Self {
            events,
            stop,
            join: Some(join),
        }
    }

    /// Take every event observed since the last drain, oldest first.
    pub fn drain(&self) -> Vec<StationEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for StationWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_station(
    settings: StationSettings,
    events: Sender<StationEvent>,
    wake: LayoutWake,
    stop: Arc<AtomicBool>,
) {
    let fail = |message: String| {
        let _ = events.send(StationEvent::Failed { message });
        wake();
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return fail(format!("could not start the station runtime: {error}"));
        }
    };
    runtime.block_on(async {
        let station = match open_station(&settings).await {
            Ok(station) => station,
            Err(message) => return fail(message),
        };
        let _ = events.send(StationEvent::Connected {
            name: settings.name.clone(),
            port: settings.port.clone(),
            expires_at_ms: station.station.lease().expires_at_ms(),
        });
        wake();

        let mut last_generation = None;
        while !stop.load(Ordering::Relaxed) {
            if let Err(error) = station.renew_if_needed() {
                return fail(format!("station grant renewal failed: {error}"));
            }
            match station.station.management_snapshot().await {
                Ok(snapshot) => {
                    let generation = snapshot.generation;
                    if last_generation != Some(generation) {
                        last_generation = Some(generation);
                        let captured_unix_ms = match unix_time_ms() {
                            Ok(now_ms) => now_ms,
                            Err(error) => {
                                return fail(format!("station clock failed: {error}"));
                            }
                        };
                        let _ = events.send(StationEvent::Snapshot {
                            snapshot: Box::new(snapshot),
                            captured_unix_ms,
                        });
                        wake();
                    }
                }
                // A renewal landed mid-operation; the station remains live.
                Err(SitedStationError::OperationInterrupted) => {}
                Err(error) => {
                    return fail(format!("station snapshot failed: {error}"));
                }
            }
            tokio::time::sleep(POLL).await;
        }
        station.station.stop().await;
    });
}

struct OpenedStation {
    station: SitedStation,
    credential: SitedStationCredential,
    device_id: DeviceId,
    data_root: PathBuf,
    label: String,
}

impl OpenedStation {
    /// Re-issue the self-granted host authority before the lease deadline.
    ///
    /// The lease itself re-reads the wallet on every station operation; this
    /// only keeps a fresh grant present so those checks keep passing.
    fn renew_if_needed(&self) -> Result<(), String> {
        let now_ms = unix_time_ms().map_err(|error| format!("station clock failed: {error}"))?;
        let expires_at_ms = self.station.lease().expires_at_ms();
        if expires_at_ms.saturating_sub(now_ms) >= RENEW_MARGIN.as_millis() as u64 {
            return Ok(());
        }
        self.credential
            .issue_remote_auth_grant(
                &self.data_root,
                self.device_id,
                self.label.clone(),
                now_ms,
                now_ms + GRANT_WINDOW.as_millis() as u64,
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

async fn open_station(settings: &StationSettings) -> Result<OpenedStation, String> {
    std::fs::create_dir_all(&settings.data_root)
        .map_err(|error| format!("station data root was not usable: {error}"))?;
    let seed = ensure_wallet_state(&settings.data_root, PersonaId::new(), "Signalman desktop")
        .map_err(|error| format!("station wallet was not usable: {error}"))?;
    let provider = InMemoryProvider::from_seed(seed);
    let device_id = durable_device_id(&settings.data_root)?;
    let credential = SitedStationCredential::derive_for_device(&provider, device_id)
        .map_err(|error| format!("station credential derivation failed: {error}"))?;
    let now_ms = unix_time_ms().map_err(|error| format!("station clock failed: {error}"))?;
    credential
        .issue_remote_auth_grant(
            &settings.data_root,
            device_id,
            settings.name.clone(),
            now_ms,
            now_ms + GRANT_WINDOW.as_millis() as u64,
        )
        .map_err(|error| format!("station grant was refused: {error}"))?;
    let station = credential
        .open_station(
            &settings.data_root,
            device_id,
            settings.port.clone(),
            settings.name.clone(),
        )
        .await
        .map_err(|error| format!("station did not open on {}: {error}", settings.port))?;
    Ok(OpenedStation {
        station,
        credential,
        device_id,
        data_root: settings.data_root.clone(),
        label: settings.name.clone(),
    })
}

/// Load or mint the durable host-side device id for this station.
///
/// The id is host bookkeeping (which grant and roster row this station uses),
/// not a secret; it sits beside the wallet as plain JSON.
fn durable_device_id(data_root: &std::path::Path) -> Result<DeviceId, String> {
    let path = data_root.join("station-device-id.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        return serde_json::from_str(&text)
            .map_err(|error| format!("station device-id record was not readable: {error}"));
    }
    let device_id = DeviceId::new();
    let text = serde_json::to_string(&device_id)
        .map_err(|error| format!("station device-id record was not writable: {error}"))?;
    std::fs::write(&path, text)
        .map_err(|error| format!("station device-id record was not writable: {error}"))?;
    Ok(device_id)
}

fn unix_time_ms() -> Result<u64, std::time::SystemTimeError> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(u64::try_from(duration.as_millis()).expect("unix millisecond count fits u64"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_durable_device_id_survives_reload() {
        let root = tempfile::tempdir().unwrap();
        let first = durable_device_id(root.path()).unwrap();
        let second = durable_device_id(root.path()).unwrap();
        assert_eq!(first, second);
    }
}
