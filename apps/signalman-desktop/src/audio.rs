//! Host audio capture and playback for Signalman voice drops.
//!
//! CPAL and its streams stay in this desktop boundary. Signalman owns the
//! checked Pipit clip and message facts; this module only turns an explicitly
//! selected host input into 8 kHz mono PCM and a decoded clip into samples for
//! an explicitly selected host output.

mod capture;
mod pcm;
mod playback;

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use signalman::voice::DecodedVoice;

use crate::network::LayoutWake;
use capture::CaptureSession;
use playback::PlaybackSession;

pub use pcm::normalize_interleaved;

pub const VOICE_SAMPLE_RATE: u32 = 8_000;
pub const MAX_CAPTURE_SECONDS: u32 = 60;
const MAX_HOST_SAMPLES: usize = 16 * 1024 * 1024;
const STREAM_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceChoice {
    pub id: String,
    pub label: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioInventory {
    pub inputs: Vec<AudioDeviceChoice>,
    pub outputs: Vec<AudioDeviceChoice>,
}

/// Enumerate stable CPAL device IDs. The operating-system default is sorted
/// first, but it remains an ordinary visible choice rather than a hidden rule.
pub fn inventory() -> Result<AudioInventory, AudioError> {
    let default_host = cpal::default_host();
    let default_input = default_host
        .default_input_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let default_output = default_host
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());

    let mut inputs = BTreeMap::new();
    let mut outputs = BTreeMap::new();
    let mut host_seen = false;
    for host_id in cpal::available_hosts() {
        let Ok(host) = cpal::host_from_id(host_id) else {
            continue;
        };
        host_seen = true;
        if let Ok(devices) = host.input_devices() {
            collect_devices(devices, default_input.as_deref(), &mut inputs);
        }
        if let Ok(devices) = host.output_devices() {
            collect_devices(devices, default_output.as_deref(), &mut outputs);
        }
    }
    if !host_seen {
        return Err(AudioError::Unavailable(
            "no host audio backend is available".into(),
        ));
    }
    Ok(AudioInventory {
        inputs: sorted_choices(inputs),
        outputs: sorted_choices(outputs),
    })
}

fn collect_devices(
    devices: impl Iterator<Item = cpal::Device>,
    default_id: Option<&str>,
    choices: &mut BTreeMap<String, AudioDeviceChoice>,
) {
    for device in devices {
        let Ok(id) = device.id() else { continue };
        let id = id.to_string();
        let label = device.to_string();
        choices.entry(id.clone()).or_insert(AudioDeviceChoice {
            is_default: default_id == Some(id.as_str()),
            id,
            label,
        });
    }
}

fn sorted_choices(choices: BTreeMap<String, AudioDeviceChoice>) -> Vec<AudioDeviceChoice> {
    let mut choices = choices.into_values().collect::<Vec<_>>();
    choices.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.id.cmp(&right.id))
    });
    choices
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioOperation {
    Capture,
    Playback,
}

impl AudioOperation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Playback => "playback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureStarted {
    pub device_id: String,
    pub device_label: String,
    pub source_sample_rate: u32,
    pub source_channels: u16,
    pub max_duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedVoice {
    pub pcm: Vec<i16>,
    pub device_id: String,
    pub device_label: String,
    pub source_sample_rate: u32,
    pub source_channels: u16,
    pub captured_duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackStarted {
    pub device_id: String,
    pub device_label: String,
    pub output_sample_rate: u32,
    pub output_channels: u16,
    pub decoded_duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackReceipt {
    pub device_id: String,
    pub device_label: String,
    pub output_sample_rate: u32,
    pub output_channels: u16,
    pub decoded_duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    CaptureStarted(CaptureStarted),
    Captured(CapturedVoice),
    PlaybackStarted(PlaybackStarted),
    PlaybackFinished(PlaybackReceipt),
    Failed {
        operation: AudioOperation,
        message: String,
    },
}

enum Command {
    StartCapture {
        device_id: String,
        max_duration: Duration,
    },
    StopCapture,
    Play {
        device_id: String,
        voice: DecodedVoice,
    },
    Stop,
}

pub struct AudioWorker {
    commands: Sender<Command>,
    events: Receiver<AudioEvent>,
    join: Option<JoinHandle<()>>,
}

impl AudioWorker {
    pub fn spawn(wake: LayoutWake) -> Result<Self, std::io::Error> {
        let (commands, receiver) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let join = thread::Builder::new()
            .name("signalman-host-audio".to_owned())
            .spawn(move || run_actor(receiver, event_tx, wake))?;
        Ok(Self {
            commands,
            events,
            join: Some(join),
        })
    }

    pub fn start_capture(&self, device_id: String, max_duration: Duration) -> bool {
        self.commands
            .send(Command::StartCapture {
                device_id,
                max_duration,
            })
            .is_ok()
    }

    pub fn stop_capture(&self) -> bool {
        self.commands.send(Command::StopCapture).is_ok()
    }

    pub fn play(&self, device_id: String, voice: DecodedVoice) -> bool {
        self.commands
            .send(Command::Play { device_id, voice })
            .is_ok()
    }

    pub fn drain(&self) -> Vec<AudioEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for AudioWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_actor(receiver: Receiver<Command>, event_tx: Sender<AudioEvent>, wake: LayoutWake) {
    let mut capture = None;
    let mut playback = None;
    let mut running = true;
    while running {
        let command = if capture.is_some() || playback.is_some() {
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            receiver.recv().ok()
        };

        match command {
            Some(Command::StartCapture {
                device_id,
                max_duration,
            }) => {
                if capture.is_some() || playback.is_some() {
                    emit(
                        &event_tx,
                        &wake,
                        AudioEvent::Failed {
                            operation: AudioOperation::Capture,
                            message: "another host audio operation is active".into(),
                        },
                    );
                } else {
                    match CaptureSession::start(&device_id, max_duration) {
                        Ok((session, started)) => {
                            capture = Some(session);
                            emit(&event_tx, &wake, AudioEvent::CaptureStarted(started));
                        }
                        Err(error) => {
                            emit_failure(&event_tx, &wake, AudioOperation::Capture, error)
                        }
                    }
                }
            }
            Some(Command::StopCapture) => {
                if let Some(session) = capture.take() {
                    finish_capture(session, &event_tx, &wake);
                }
            }
            Some(Command::Play { device_id, voice }) => {
                if capture.is_some() || playback.is_some() {
                    emit(
                        &event_tx,
                        &wake,
                        AudioEvent::Failed {
                            operation: AudioOperation::Playback,
                            message: "another host audio operation is active".into(),
                        },
                    );
                } else {
                    match PlaybackSession::start(&device_id, voice) {
                        Ok((session, started)) => {
                            playback = Some(session);
                            emit(&event_tx, &wake, AudioEvent::PlaybackStarted(started));
                        }
                        Err(error) => {
                            emit_failure(&event_tx, &wake, AudioOperation::Playback, error)
                        }
                    }
                }
            }
            Some(Command::Stop) => running = false,
            None => {}
        }

        if capture.as_ref().is_some_and(CaptureSession::is_finished) {
            finish_capture(
                capture.take().expect("capture was present"),
                &event_tx,
                &wake,
            );
        }
        if playback.as_ref().is_some_and(PlaybackSession::is_finished) {
            let session = playback.take().expect("playback was present");
            match session.finish() {
                Ok(receipt) => emit(&event_tx, &wake, AudioEvent::PlaybackFinished(receipt)),
                Err(error) => emit_failure(&event_tx, &wake, AudioOperation::Playback, error),
            }
        }
    }
}

fn finish_capture(session: CaptureSession, event_tx: &Sender<AudioEvent>, wake: &LayoutWake) {
    match session.finish() {
        Ok(captured) => emit(event_tx, wake, AudioEvent::Captured(captured)),
        Err(error) => emit_failure(event_tx, wake, AudioOperation::Capture, error),
    }
}

fn emit_failure(
    event_tx: &Sender<AudioEvent>,
    wake: &LayoutWake,
    operation: AudioOperation,
    error: AudioError,
) {
    emit(
        event_tx,
        wake,
        AudioEvent::Failed {
            operation,
            message: error.to_string(),
        },
    );
}

fn emit(event_tx: &Sender<AudioEvent>, wake: &LayoutWake, event: AudioEvent) {
    if event_tx.send(event).is_ok() {
        wake();
    }
}

fn device_by_id(id: &str) -> Result<cpal::Device, AudioError> {
    let id = id
        .parse::<cpal::DeviceId>()
        .map_err(|error| AudioError::Device(error.to_string()))?;
    let host =
        cpal::host_from_id(id.host()).map_err(|error| AudioError::Device(error.to_string()))?;
    host.device_by_id(&id)
        .ok_or_else(|| AudioError::Unavailable(format!("audio device {id} is unavailable")))
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("{0}")]
    Unavailable(String),
    #[error("audio device error: {0}")]
    Device(String),
    #[error("audio stream error: {0}")]
    Stream(String),
    #[error("audio sample format {0} is unsupported")]
    SampleFormat(String),
    #[error("voice capture duration must be between 1 ms and 60 seconds")]
    InvalidDuration,
    #[error("audio channel count and sample rate must be nonzero")]
    InvalidFormat,
    #[error(
        "audio input format {sample_rate} Hz by {channels} channels exceeds the bounded capture buffer"
    )]
    DeviceFormatTooLarge { sample_rate: u32, channels: u16 },
    #[error("decoded voice exceeds the bounded host output buffer")]
    OutputTooLarge,
    #[error("host audio ended on an incomplete interleaved frame")]
    IncompleteFrame,
    #[error("host audio contained no samples")]
    EmptyPcm,
    #[error("the real-time input callback dropped {0} samples")]
    DroppedInput(usize),
}
