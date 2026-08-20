//! Live sealed-station adapter for the desktop host.
//!
//! The Tokio runtime and the delegated station identity stay on one ordinary
//! worker thread. The UI receives typed facts and durable message events; it
//! never receives the station handle or a private identity.

use std::ffi::OsString;
use std::path::{Component, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mere_signalman::{RunningSitedStationHead, SitedStationHead, SitedStationHeadError};
use personae::{SealedRecordStorage, load_or_create_auto_unlock_root};
use signalman::management::{ManagementGeneration, ManagementSnapshot};
use signalman::message::{
    Message, MessageEvent, MessageId, MessageObservation, MessagePeer, observe_station_event,
    sent_event,
};
use tokio::runtime::Builder;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::time::MissedTickBehavior;

use crate::network::LayoutWake;

const DATA_ROOT_ENV: &str = "SIGNALMAN_STATION_DATA_ROOT";
const RECORD_ENV: &str = "SIGNALMAN_STATION_RECORD";
const PORT_ENV: &str = "SIGNALMAN_STATION_PORT";
const NAME_ENV: &str = "SIGNALMAN_STATION_NAME";
const PATIENCE_ENV: &str = "SIGNALMAN_STATION_SEND_PATIENCE_SECONDS";
const DEFAULT_SEND_PATIENCE: Duration = Duration::from_secs(30);
const SNAPSHOT_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationStartupConfig {
    data_root: PathBuf,
    record_path: PathBuf,
    port: String,
    name: String,
    send_patience: Duration,
}

impl StationStartupConfig {
    /// Read the explicit staging attachment. With no station variables the
    /// desktop stays disconnected; a partial attachment is an error shown by
    /// the face rather than a guessed default.
    pub fn from_env() -> Result<Option<Self>, StationConfigError> {
        Self::from_values([
            std::env::var_os(DATA_ROOT_ENV),
            std::env::var_os(RECORD_ENV),
            std::env::var_os(PORT_ENV),
            std::env::var_os(NAME_ENV),
            std::env::var_os(PATIENCE_ENV),
        ])
    }

    fn from_values(values: [Option<OsString>; 5]) -> Result<Option<Self>, StationConfigError> {
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        let names = [DATA_ROOT_ENV, RECORD_ENV, PORT_ENV, NAME_ENV];
        let missing = names
            .iter()
            .zip(values.iter())
            .filter_map(|(name, value)| value.is_none().then_some(*name))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(StationConfigError::Partial(missing.join(", ")));
        }
        let data_root = PathBuf::from(values[0].clone().expect("checked above"));
        let record_path = PathBuf::from(values[1].clone().expect("checked above"));
        if data_root.as_os_str().is_empty() {
            return Err(StationConfigError::DataRoot);
        }
        if record_path.is_absolute()
            || !record_path
                .components()
                .any(|component| matches!(component, Component::Normal(_)))
            || record_path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(StationConfigError::RecordPath);
        }
        let port = required_text(PORT_ENV, values[2].clone().expect("checked above"))?;
        let name = required_text(NAME_ENV, values[3].clone().expect("checked above"))?;
        let send_patience = match values[4].as_ref() {
            Some(value) => {
                let value = value
                    .to_str()
                    .ok_or(StationConfigError::Patience)?
                    .parse::<u64>()
                    .map_err(|_| StationConfigError::Patience)?;
                if value == 0 {
                    return Err(StationConfigError::Patience);
                }
                Duration::from_secs(value)
            }
            None => DEFAULT_SEND_PATIENCE,
        };
        Ok(Some(Self {
            data_root,
            record_path,
            port,
            name,
            send_patience,
        }))
    }
}

fn required_text(name: &'static str, value: OsString) -> Result<String, StationConfigError> {
    let value = value
        .into_string()
        .map_err(|_| StationConfigError::Text(name))?;
    if value.trim().is_empty() {
        return Err(StationConfigError::Text(name));
    }
    Ok(value.trim().to_owned())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StationConfigError {
    #[error("station attachment is partial; also set {0}")]
    Partial(String),
    #[error("SIGNALMAN_STATION_RECORD must be a relative sealed-record path")]
    RecordPath,
    #[error("SIGNALMAN_STATION_DATA_ROOT must name a data directory")]
    DataRoot,
    #[error("{0} must contain non-empty Unicode text")]
    Text(&'static str),
    #[error("SIGNALMAN_STATION_SEND_PATIENCE_SECONDS must be a positive whole number")]
    Patience,
}

#[derive(Clone, Debug)]
pub struct StationRequest {
    pub message: Message,
}

impl StationRequest {
    pub fn id(&self) -> MessageId {
        self.message.id()
    }
}

#[derive(Clone, Debug)]
pub enum StationEvent {
    Connected {
        local: MessagePeer,
    },
    Management {
        snapshot: Box<ManagementSnapshot>,
        captured_unix_ms: u64,
    },
    Message(Box<MessageEvent>),
    PeerAppeared {
        destination: MessagePeer,
        name: Option<String>,
    },
    Dropped(String),
    Failed {
        id: Option<MessageId>,
        message: String,
    },
    Disconnected(String),
}

enum StationCommand {
    Send(Box<StationRequest>),
    Stop,
}

pub struct StationWorker {
    commands: tokio_mpsc::UnboundedSender<StationCommand>,
    events: mpsc::Receiver<StationEvent>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StationWorker {
    pub fn spawn(config: StationStartupConfig, wake: LayoutWake) -> std::io::Result<Self> {
        let (commands, command_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("signalman-station".into())
            .spawn(move || run_thread(config, command_rx, event_tx, wake))?;
        Ok(Self {
            commands,
            events,
            thread: Some(thread),
        })
    }

    pub fn send(&self, request: StationRequest) -> bool {
        self.commands
            .send(StationCommand::Send(Box::new(request)))
            .is_ok()
    }

    pub fn drain(&self) -> Vec<StationEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for StationWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(StationCommand::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_thread(
    config: StationStartupConfig,
    commands: tokio_mpsc::UnboundedReceiver<StationCommand>,
    events: mpsc::Sender<StationEvent>,
    wake: LayoutWake,
) {
    let runtime = match Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            emit(
                &events,
                &wake,
                StationEvent::Disconnected(format!("could not start the station runtime: {error}")),
            );
            return;
        }
    };
    runtime.block_on(run_station(config, commands, events, wake));
}

async fn run_station(
    config: StationStartupConfig,
    mut commands: tokio_mpsc::UnboundedReceiver<StationCommand>,
    events: mpsc::Sender<StationEvent>,
    wake: LayoutWake,
) {
    let running = match open_station(&config).await {
        Ok(running) => running,
        Err(error) => {
            emit(
                &events,
                &wake,
                StationEvent::Disconnected(error.to_string()),
            );
            return;
        }
    };
    let local = match running.address().await {
        Ok(address) => MessagePeer::new(
            *address.as_bytes(),
            Some(*running.head().public_identity().ed25519_bytes()),
        ),
        Err(error) => {
            emit(
                &events,
                &wake,
                StationEvent::Disconnected(error.to_string()),
            );
            running.stop().await;
            return;
        }
    };
    emit(&events, &wake, StationEvent::Connected { local });

    let mut last_generation = None;
    capture_management(&running, &events, &wake, &mut last_generation).await;
    let mut poll = tokio::time::interval(SNAPSHOT_POLL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(StationCommand::Send(request)) => {
                    send_message(&running, *request, config.send_patience, &events, &wake).await;
                    capture_management(&running, &events, &wake, &mut last_generation).await;
                }
                Some(StationCommand::Stop) | None => {
                    running.stop().await;
                    return;
                }
            },
            event = running.next_event() => match event {
                Ok(event) => {
                    emit_observation(&event, local, &events, &wake);
                    capture_management(&running, &events, &wake, &mut last_generation).await;
                }
                Err(SitedStationHeadError::OperationInterrupted) => {}
                Err(error) => {
                    emit(&events, &wake, StationEvent::Disconnected(error.to_string()));
                    return;
                }
            },
            _ = poll.tick() => {
                capture_management(&running, &events, &wake, &mut last_generation).await;
            }
        }
    }
}

async fn open_station(
    config: &StationStartupConfig,
) -> Result<RunningSitedStationHead, StationOpenError> {
    let unlock_path = pandect::wallet_store::identity_auto_unlock_root_path(&config.data_root);
    if !unlock_path.is_file() {
        return Err(StationOpenError::MissingUnlock(unlock_path));
    }
    let key = load_or_create_auto_unlock_root(&unlock_path)?
        .ok_or(StationOpenError::AutoUnlockUnavailable)?;
    let storage = SealedRecordStorage::open_with_key(&config.data_root, key);
    let head = SitedStationHead::restore(storage, &config.record_path)?;
    head.open_station(&config.port, &config.name)
        .await
        .map_err(Into::into)
}

async fn send_message(
    running: &RunningSitedStationHead,
    request: StationRequest,
    patience: Duration,
    events: &mpsc::Sender<StationEvent>,
    wake: &LayoutWake,
) {
    let id = request.id();
    let payload = match request.message.encode_payload(unix_ms() as f64 / 1_000.0) {
        Ok(payload) => payload,
        Err(error) => {
            emit(
                events,
                wake,
                StationEvent::Failed {
                    id: Some(id),
                    message: error.to_string(),
                },
            );
            return;
        }
    };
    let destination = request.message.recipient().address().to_string();
    loop {
        match running.send_payload(&destination, &payload, patience).await {
            Ok(sent) => {
                emit(
                    events,
                    wake,
                    StationEvent::Message(Box::new(sent_event(id, &sent, unix_ms()))),
                );
                return;
            }
            Err(SitedStationHeadError::OperationInterrupted) => continue,
            Err(error) => {
                emit(
                    events,
                    wake,
                    StationEvent::Failed {
                        id: Some(id),
                        message: error.to_string(),
                    },
                );
                return;
            }
        }
    }
}

fn emit_observation(
    event: &postilion::Event,
    local: MessagePeer,
    events: &mpsc::Sender<StationEvent>,
    wake: &LayoutWake,
) {
    match observe_station_event(event, local, unix_ms()) {
        Ok(MessageObservation::Incoming(event)) => {
            emit(events, wake, StationEvent::Message(Box::new(event)));
        }
        Ok(MessageObservation::PeerAppeared { destination, name }) => {
            emit(
                events,
                wake,
                StationEvent::PeerAppeared { destination, name },
            );
        }
        Ok(MessageObservation::Dropped(message)) => {
            emit(events, wake, StationEvent::Dropped(message));
        }
        Err(error) => emit(
            events,
            wake,
            StationEvent::Dropped(format!(
                "authenticated message was refused by Signalman: {error}"
            )),
        ),
    }
}

async fn capture_management(
    running: &RunningSitedStationHead,
    events: &mpsc::Sender<StationEvent>,
    wake: &LayoutWake,
    last_generation: &mut Option<ManagementGeneration>,
) {
    match running.management_snapshot().await {
        Ok(snapshot) if Some(snapshot.generation) != *last_generation => {
            *last_generation = Some(snapshot.generation);
            emit(
                events,
                wake,
                StationEvent::Management {
                    snapshot: Box::new(snapshot),
                    captured_unix_ms: unix_ms(),
                },
            );
        }
        Ok(_) | Err(SitedStationHeadError::OperationInterrupted) => {}
        Err(error) => emit(
            events,
            wake,
            StationEvent::Failed {
                id: None,
                message: format!("station management snapshot failed: {error}"),
            },
        ),
    }
}

fn emit(events: &mpsc::Sender<StationEvent>, wake: &LayoutWake, event: StationEvent) {
    if events.send(event).is_ok() {
        wake();
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[derive(Debug, thiserror::Error)]
enum StationOpenError {
    #[error("the station auto-unlock record does not exist at {0}")]
    MissingUnlock(PathBuf),
    #[error("this host cannot unlock an AutoOs station record")]
    AutoUnlockUnavailable,
    #[error(transparent)]
    Identity(#[from] personae::IdentityError),
    #[error(transparent)]
    Station(#[from] SitedStationHeadError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(
        data_root: Option<&str>,
        record: Option<&str>,
        port: Option<&str>,
        name: Option<&str>,
        patience: Option<&str>,
    ) -> [Option<OsString>; 5] {
        [data_root, record, port, name, patience].map(|value| value.map(OsString::from))
    }

    #[test]
    fn absent_attachment_stays_disconnected() {
        assert_eq!(
            StationStartupConfig::from_values(values(None, None, None, None, None)).unwrap(),
            None
        );
    }

    #[test]
    fn partial_or_escaping_attachment_is_refused() {
        assert!(matches!(
            StationStartupConfig::from_values(values(
                Some("root"),
                Some("identity/station.json"),
                Some("COM6"),
                None,
                None
            )),
            Err(StationConfigError::Partial(_))
        ));
        assert_eq!(
            StationStartupConfig::from_values(values(
                Some("root"),
                Some("../station.json"),
                Some("COM6"),
                Some("field"),
                None
            )),
            Err(StationConfigError::RecordPath)
        );
        assert_eq!(
            StationStartupConfig::from_values(values(
                Some(""),
                Some("station.json"),
                Some("COM6"),
                Some("field"),
                None
            )),
            Err(StationConfigError::DataRoot)
        );
    }

    #[test]
    fn complete_attachment_keeps_owner_patience() {
        let config = StationStartupConfig::from_values(values(
            Some("root"),
            Some("identity/station.json"),
            Some("COM6"),
            Some("field"),
            Some("45"),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(config.send_patience, Duration::from_secs(45));
    }
}
