//! Host output: one decoded clip played over a CPAL output stream.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, I24, SampleFormat, SizedSample, U24};
use signalman::voice::DecodedVoice;

use super::pcm::resample_mono;
use super::{
    AudioError, MAX_HOST_SAMPLES, PlaybackReceipt, PlaybackStarted, STREAM_TIMEOUT, device_by_id,
};

pub(super) struct PlaybackSession {
    _stream: cpal::Stream,
    finished: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
    receipt: PlaybackReceipt,
}

impl PlaybackSession {
    pub(super) fn start(
        device_id: &str,
        voice: DecodedVoice,
    ) -> Result<(Self, PlaybackStarted), AudioError> {
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

    pub(super) fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire) || self.stream_error.lock().unwrap().is_some()
    }

    pub(super) fn finish(self) -> Result<PlaybackReceipt, AudioError> {
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
