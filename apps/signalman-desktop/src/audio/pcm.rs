//! PCM shaping between host formats and Pipit's 8 kHz mono.

use super::{AudioError, VOICE_SAMPLE_RATE};

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

pub(super) fn resample_mono(
    input: &[f32],
    input_rate: u32,
    output_rate: u32,
) -> Result<Vec<f32>, AudioError> {
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
