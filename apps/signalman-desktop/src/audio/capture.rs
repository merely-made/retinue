//! Host input: one bounded capture session over a CPAL input stream.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, I24, Sample, SampleFormat, SizedSample, U24};

use super::{
    AudioError, CaptureStarted, CapturedVoice, MAX_CAPTURE_SECONDS, MAX_HOST_SAMPLES,
    STREAM_TIMEOUT, VOICE_SAMPLE_RATE, device_by_id, normalize_interleaved,
};

pub(super) struct CaptureSession {
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
    pub(super) fn start(
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

    pub(super) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire) || self.stream_error.lock().unwrap().is_some()
    }

    pub(super) fn finish(self) -> Result<CapturedVoice, AudioError> {
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
