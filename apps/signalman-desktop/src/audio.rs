//! Host audio capture and playback for Signalman voice drops.
//!
//! CPAL and its streams stay in this desktop boundary. Signalman owns the
//! checked Pipit clip and message facts; this module only turns an explicitly
//! selected host input into 8 kHz mono PCM and a decoded clip into samples for
//! an explicitly selected host output.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, I24, Sample, SampleFormat, SizedSample, U24};
use signalman::voice::DecodedVoice;

use crate::network::LayoutWake;

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

struct CaptureSession {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    finished: Arc<AtomicBool>,
    dropped_samples: Arc<AtomicUsize>,
    stream_error: Arc<Mutex<Option<String>>>,
    device_id: String,
    device_label: String,
    sample_rate: u32,
    channels: u16,
}

impl CaptureSession {
    fn start(
        device_id: &str,
        max_duration: Duration,
    ) -> Result<(Self, CaptureStarted), AudioError> {
        if max_duration.is_zero() || max_duration > Duration::from_secs(MAX_CAPTURE_SECONDS.into())
        {
            return Err(AudioError::InvalidDuration);
        }
        let device = device_by_id(device_id)?;
        let device_label = device.to_string();
        let supported = device
            .default_input_config()
            .map_err(|error| AudioError::Device(error.to_string()))?;
        let sample_rate = supported.sample_rate();
        let channels = supported.channels();
        let max_frames =
            u64::from(sample_rate).saturating_mul(max_duration.as_millis() as u64) / 1_000;
        let max_samples =
            usize::try_from(max_frames.saturating_mul(u64::from(channels))).unwrap_or(usize::MAX);
        if max_samples > MAX_HOST_SAMPLES {
            return Err(AudioError::DeviceFormatTooLarge {
                sample_rate,
                channels,
            });
        }
        let samples = Arc::new(Mutex::new(Vec::with_capacity(max_samples.min(64 * 1024))));
        let finished = Arc::new(AtomicBool::new(false));
        let dropped_samples = Arc::new(AtomicUsize::new(0));
        let stream_error = Arc::new(Mutex::new(None));
        let stream = build_input_stream(
            &device,
            supported,
            Arc::clone(&samples),
            max_samples,
            Arc::clone(&finished),
            Arc::clone(&dropped_samples),
            Arc::clone(&stream_error),
        )?;
        stream
            .play()
            .map_err(|error| AudioError::Stream(error.to_string()))?;
        let max_duration_ms = u32::try_from(max_duration.as_millis()).unwrap_or(u32::MAX);
        let started = CaptureStarted {
            device_id: device_id.to_owned(),
            device_label: device_label.clone(),
            source_sample_rate: sample_rate,
            source_channels: channels,
            max_duration_ms,
        };
        Ok((
            Self {
                _stream: stream,
                samples,
                finished,
                dropped_samples,
                stream_error,
                device_id: device_id.to_owned(),
                device_label,
                sample_rate,
                channels,
            },
            started,
        ))
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire) || self.stream_error.lock().unwrap().is_some()
    }

    fn finish(self) -> Result<CapturedVoice, AudioError> {
        drop(self._stream);
        if let Some(error) = self.stream_error.lock().unwrap().take() {
            return Err(AudioError::Stream(error));
        }
        let dropped = self.dropped_samples.load(Ordering::Acquire);
        if dropped > 0 {
            return Err(AudioError::DroppedInput(dropped));
        }
        let samples = std::mem::take(&mut *self.samples.lock().unwrap());
        let pcm = normalize_interleaved(&samples, self.channels, self.sample_rate)?;
        let captured_duration_ms =
            u32::try_from(pcm.len() as u64 * 1_000 / u64::from(VOICE_SAMPLE_RATE))
                .unwrap_or(u32::MAX);
        Ok(CapturedVoice {
            pcm,
            device_id: self.device_id,
            device_label: self.device_label,
            source_sample_rate: self.sample_rate,
            source_channels: self.channels,
            captured_duration_ms,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn build_input_stream(
    device: &cpal::Device,
    supported: cpal::SupportedStreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    finished: Arc<AtomicBool>,
    dropped_samples: Arc<AtomicUsize>,
    stream_error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, AudioError> {
    macro_rules! build {
        ($sample:ty) => {
            build_input_stream_for::<$sample>(
                device,
                supported.config(),
                samples,
                max_samples,
                finished,
                dropped_samples,
                stream_error,
            )
        };
    }
    match supported.sample_format() {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U24 => build!(U24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        format => Err(AudioError::SampleFormat(format.to_string())),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_input_stream_for<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    finished: Arc<AtomicBool>,
    dropped_samples: Arc<AtomicUsize>,
    stream_error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + Copy,
    f32: FromSample<T>,
{
    let error_slot = Arc::clone(&stream_error);
    device
        .build_input_stream::<T, _, _>(
            config,
            move |input, _| {
                let Ok(mut samples) = samples.try_lock() else {
                    dropped_samples.fetch_add(input.len(), Ordering::Relaxed);
                    return;
                };
                let room = max_samples.saturating_sub(samples.len());
                samples.extend(input.iter().take(room).copied().map(f32::from_sample));
                if samples.len() == max_samples {
                    finished.store(true, Ordering::Release);
                }
            },
            move |error| {
                *error_slot.lock().unwrap() = Some(error.to_string());
            },
            Some(STREAM_TIMEOUT),
        )
        .map_err(|error| AudioError::Stream(error.to_string()))
}

struct PlaybackSession {
    _stream: cpal::Stream,
    finished: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
    receipt: PlaybackReceipt,
}

impl PlaybackSession {
    fn start(device_id: &str, voice: DecodedVoice) -> Result<(Self, PlaybackStarted), AudioError> {
        if voice.pcm.is_empty() || voice.sample_rate == 0 {
            return Err(AudioError::EmptyPcm);
        }
        let device = device_by_id(device_id)?;
        let device_label = device.to_string();
        let supported = device
            .default_output_config()
            .map_err(|error| AudioError::Device(error.to_string()))?;
        let output_sample_rate = supported.sample_rate();
        let output_channels = supported.channels();
        let mono = voice
            .pcm
            .iter()
            .map(|sample| f32::from(*sample) / f32::from(i16::MAX))
            .collect::<Vec<_>>();
        let mono = resample_mono(&mono, voice.sample_rate, output_sample_rate)?;
        let output_samples = mono.len().saturating_mul(output_channels.into());
        if output_samples > MAX_HOST_SAMPLES {
            return Err(AudioError::OutputTooLarge);
        }
        let mut output = Vec::with_capacity(output_samples);
        for sample in mono {
            output.extend(std::iter::repeat_n(sample, output_channels.into()));
        }
        let cursor = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let stream_error = Arc::new(Mutex::new(None));
        let stream = build_output_stream(
            &device,
            supported,
            Arc::new(output),
            Arc::clone(&cursor),
            Arc::clone(&finished),
            Arc::clone(&stream_error),
        )?;
        stream
            .play()
            .map_err(|error| AudioError::Stream(error.to_string()))?;
        let receipt = PlaybackReceipt {
            device_id: device_id.to_owned(),
            device_label: device_label.clone(),
            output_sample_rate,
            output_channels,
            decoded_duration_ms: voice.decoded_duration_ms,
        };
        let started = PlaybackStarted {
            device_id: device_id.to_owned(),
            device_label,
            output_sample_rate,
            output_channels,
            decoded_duration_ms: voice.decoded_duration_ms,
        };
        Ok((
            Self {
                _stream: stream,
                finished,
                stream_error,
                receipt,
            },
            started,
        ))
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire) || self.stream_error.lock().unwrap().is_some()
    }

    fn finish(self) -> Result<PlaybackReceipt, AudioError> {
        drop(self._stream);
        if let Some(error) = self.stream_error.lock().unwrap().take() {
            return Err(AudioError::Stream(error));
        }
        Ok(self.receipt)
    }
}

fn build_output_stream(
    device: &cpal::Device,
    supported: cpal::SupportedStreamConfig,
    samples: Arc<Vec<f32>>,
    cursor: Arc<AtomicUsize>,
    finished: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, AudioError> {
    macro_rules! build {
        ($sample:ty) => {
            build_output_stream_for::<$sample>(
                device,
                supported.config(),
                samples,
                cursor,
                finished,
                stream_error,
            )
        };
    }
    match supported.sample_format() {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U24 => build!(U24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        format => Err(AudioError::SampleFormat(format.to_string())),
    }
}

fn build_output_stream_for<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Arc<Vec<f32>>,
    cursor: Arc<AtomicUsize>,
    finished: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let error_slot = Arc::clone(&stream_error);
    device
        .build_output_stream::<T, _, _>(
            config,
            move |output, _| {
                let start = cursor.fetch_add(output.len(), Ordering::AcqRel);
                for (offset, target) in output.iter_mut().enumerate() {
                    let sample = samples
                        .get(start.saturating_add(offset))
                        .copied()
                        .unwrap_or(0.0);
                    *target = T::from_sample(sample);
                }
                if start.saturating_add(output.len()) >= samples.len() {
                    finished.store(true, Ordering::Release);
                }
            },
            move |error| {
                *error_slot.lock().unwrap() = Some(error.to_string());
            },
            Some(STREAM_TIMEOUT),
        )
        .map_err(|error| AudioError::Stream(error.to_string()))
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

/// Downmix interleaved host PCM and resample it to Pipit's 8 kHz mono input.
pub fn normalize_interleaved(
    interleaved: &[f32],
    channels: u16,
    sample_rate: u32,
) -> Result<Vec<i16>, AudioError> {
    if channels == 0 || sample_rate == 0 {
        return Err(AudioError::InvalidFormat);
    }
    if interleaved.is_empty() {
        return Err(AudioError::EmptyPcm);
    }
    let channels = usize::from(channels);
    if !interleaved.len().is_multiple_of(channels) {
        return Err(AudioError::IncompleteFrame);
    }
    let mono = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect::<Vec<_>>();
    let mono = resample_mono(&mono, sample_rate, VOICE_SAMPLE_RATE)?;
    if mono.is_empty() {
        return Err(AudioError::EmptyPcm);
    }
    Ok(mono
        .into_iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
        .collect())
}

fn resample_mono(input: &[f32], input_rate: u32, output_rate: u32) -> Result<Vec<f32>, AudioError> {
    if input_rate == 0 || output_rate == 0 {
        return Err(AudioError::InvalidFormat);
    }
    if input.is_empty() || input_rate == output_rate {
        return Ok(input.to_vec());
    }
    let output_len = usize::try_from(
        (input.len() as u64).saturating_mul(u64::from(output_rate)) / u64::from(input_rate),
    )
    .unwrap_or(usize::MAX);
    if output_len == 0 {
        return Ok(Vec::new());
    }
    if output_rate > input_rate {
        let scale = input_rate as f64 / output_rate as f64;
        return Ok((0..output_len)
            .map(|index| {
                let position = index as f64 * scale;
                let left = position.floor() as usize;
                let right = (left + 1).min(input.len() - 1);
                let fraction = (position - left as f64) as f32;
                input[left] * (1.0 - fraction) + input[right] * fraction
            })
            .collect());
    }

    // A box average is deliberately used for downsampling host audio. It is a
    // small anti-alias filter, unlike simply selecting every sixth 48 kHz frame.
    let scale = input_rate as f64 / output_rate as f64;
    Ok((0..output_len)
        .map(|index| {
            let start = index as f64 * scale;
            let end = (index + 1) as f64 * scale;
            let mut cursor = start;
            let mut sum = 0.0_f64;
            while cursor < end {
                let source = (cursor.floor() as usize).min(input.len() - 1);
                let boundary = end.min(source as f64 + 1.0);
                let weight = boundary - cursor;
                sum += f64::from(input[source]) * weight;
                cursor = boundary;
            }
            (sum / (end - start)) as f32
        })
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_48k_downmixes_and_resamples_to_exact_8k_duration() {
        let mut input = Vec::new();
        for frame in 0..48_000 {
            let sample = if frame % 480 < 240 { 0.5 } else { -0.5 };
            input.extend([sample, sample]);
        }
        let pcm = normalize_interleaved(&input, 2, 48_000).unwrap();
        assert_eq!(pcm.len(), 8_000);
        assert!(pcm.iter().any(|sample| *sample > 10_000));
        assert!(pcm.iter().any(|sample| *sample < -10_000));
    }

    #[test]
    fn downmix_uses_every_channel_and_refuses_partial_frames() {
        let pcm = normalize_interleaved(&[1.0, -1.0, 0.5, 0.5], 2, 8_000).unwrap();
        assert_eq!(pcm, [0, 16_384]);
        assert!(matches!(
            normalize_interleaved(&[1.0, 0.0, 0.5], 2, 8_000),
            Err(AudioError::IncompleteFrame)
        ));
    }

    #[test]
    fn upsample_retains_duration_and_endpoints() {
        let output = resample_mono(&[-1.0, 1.0], 8_000, 48_000).unwrap();
        assert_eq!(output.len(), 12);
        assert_eq!(output[0], -1.0);
        assert!(output[3].abs() < f32::EPSILON);
        assert_eq!(*output.last().unwrap(), 1.0);
    }
}
