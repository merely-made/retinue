//! File-backed voice clips and their receipt facts.
//!
//! Pipit owns the clip and Outrider owns its LXMF field. Signalman owns the
//! operator-facing facts, file policy, and decoded material.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use outrider::LxmfPayload;
use outrider::voice::{AM_CUSTOM, FieldKey};
use retinue::endpoint::PayloadMode;
use serde::{Deserialize, Serialize};

/// A caller-selected Pipit encoding. Signalman does not silently choose an
/// airtime/quality tradeoff for the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceEncoding {
    ImaAdpcm,
    Lpc10,
    Lpc10Half,
    Other(u8),
}

impl VoiceEncoding {
    fn params(self) -> Result<pipit::ClipParams, VoiceClipError> {
        Ok(match self {
            Self::ImaAdpcm => pipit::ClipParams::adpcm(),
            Self::Lpc10 => pipit::ClipParams::lpc10(),
            Self::Lpc10Half => pipit::ClipParams::lpc10_half(),
            Self::Other(id) => return Err(VoiceClipError::UnsupportedEncoding(id)),
        })
    }

    fn from_codec(codec: pipit::Codec) -> Self {
        match codec {
            pipit::Codec::ImaAdpcm => Self::ImaAdpcm,
            pipit::Codec::Lpc10 => Self::Lpc10,
            pipit::Codec::Lpc10Half => Self::Lpc10Half,
            other => Self::Other(other.id()),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ImaAdpcm => "Pipit IMA ADPCM",
            Self::Lpc10 => "Pipit LPC-10",
            Self::Lpc10Half => "Pipit LPC-10 half-rate",
            Self::Other(_) => "Unknown Pipit codec",
        }
    }
}

/// Facts read from the self-describing clip, rather than inferred from the
/// file name or selected encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceClipFacts {
    pub encoding: VoiceEncoding,
    pub sample_rate: u32,
    pub duration_ms: u32,
    pub encoded_bytes: usize,
}

/// One checked Pipit clip suitable for a Signalman message log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceClip {
    encoded: Vec<u8>,
    facts: VoiceClipFacts,
}

impl VoiceClip {
    /// Read bounded signed 16-bit little-endian mono PCM and encode it once.
    pub fn encode_pcm16le_file(
        path: impl AsRef<Path>,
        encoding: VoiceEncoding,
        max_pcm_bytes: usize,
    ) -> Result<Self, VoiceClipError> {
        if !max_pcm_bytes.is_multiple_of(2) {
            return Err(VoiceClipError::OddPcmLimit(max_pcm_bytes));
        }
        let file = File::open(path).map_err(VoiceClipError::Read)?;
        let read_limit = u64::try_from(max_pcm_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::with_capacity(max_pcm_bytes.min(64 * 1024));
        file.take(read_limit)
            .read_to_end(&mut bytes)
            .map_err(VoiceClipError::Read)?;
        if bytes.len() > max_pcm_bytes {
            return Err(VoiceClipError::PcmTooLarge {
                bytes: bytes.len(),
                limit: max_pcm_bytes,
            });
        }
        if !bytes.len().is_multiple_of(2) {
            return Err(VoiceClipError::OddPcmBytes(bytes.len()));
        }
        let pcm = bytes
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect::<Vec<_>>();
        Self::encode_pcm(&pcm, encoding)
    }

    pub fn encode_pcm(pcm: &[i16], encoding: VoiceEncoding) -> Result<Self, VoiceClipError> {
        if pcm.is_empty() {
            return Err(VoiceClipError::EmptyPcm);
        }
        let encoded =
            pipit::encode_clip(pcm, encoding.params()?).map_err(|_| VoiceClipError::Encode)?;
        Self::from_encoded(encoded)
    }

    pub fn from_encoded(encoded: Vec<u8>) -> Result<Self, VoiceClipError> {
        let info = outrider::voice::describe(&encoded).map_err(VoiceClipError::Clip)?;
        let encoded = encoded[..info.bytes].to_vec();
        Ok(Self {
            facts: facts(info),
            encoded,
        })
    }

    pub fn encoded(&self) -> &[u8] {
        &self.encoded
    }

    pub fn facts(&self) -> VoiceClipFacts {
        self.facts
    }

    pub fn validate(&self) -> Result<(), VoiceClipError> {
        let info = outrider::voice::describe(&self.encoded).map_err(VoiceClipError::Clip)?;
        if info.bytes != self.encoded.len() || facts(info) != self.facts {
            return Err(VoiceClipError::FactMismatch);
        }
        Ok(())
    }

    /// Attach exactly one custom-codec audio field using Outrider's captured
    /// LXMF vocabulary.
    pub fn attach(&self, payload: &mut LxmfPayload) -> Result<(), VoiceClipError> {
        self.validate()?;
        let attached = outrider::voice::attach(payload, FieldKey::AUDIO, &self.encoded)
            .map_err(VoiceClipError::Clip)?;
        if facts(attached) != self.facts {
            return Err(VoiceClipError::FactMismatch);
        }
        Ok(())
    }

    /// Recover only Pipit under LXMF's honest custom-audio mode.
    pub fn from_payload(payload: &LxmfPayload) -> Result<Self, VoiceClipError> {
        let (mode, encoded) = outrider::voice::audio_at(payload, FieldKey::AUDIO)
            .ok_or(VoiceClipError::MissingAudioField)?;
        if mode != AM_CUSTOM {
            return Err(VoiceClipError::UnsupportedAudioMode(mode));
        }
        Self::from_encoded(encoded.to_vec())
    }

    pub fn decode(&self) -> Result<DecodedVoice, VoiceClipError> {
        self.validate()?;
        let (header, pcm) =
            pipit::decode_clip(&self.encoded).map_err(|_| VoiceClipError::Decode)?;
        let decoded_duration_ms = (pcm.len() as u64 * 1000 / header.sample_rate as u64) as u32;
        Ok(DecodedVoice {
            pcm,
            sample_rate: header.sample_rate,
            decoded_duration_ms,
        })
    }

    /// Join clip and decode facts to the observed carrier mode. This is the
    /// receipt a host can persist or show without reconstructing any field.
    pub fn receipt(
        &self,
        transfer_mode: PayloadMode,
        decoded: &DecodedVoice,
    ) -> Result<VoiceReceipt, VoiceClipError> {
        self.validate()?;
        if decoded.sample_rate != self.facts.sample_rate
            || decoded.decoded_duration_ms != self.facts.duration_ms
        {
            return Err(VoiceClipError::FactMismatch);
        }
        Ok(VoiceReceipt {
            encoding: self.facts.encoding,
            sample_rate: self.facts.sample_rate,
            encoded_duration_ms: self.facts.duration_ms,
            encoded_bytes: self.facts.encoded_bytes,
            transfer_mode,
            decoded_duration_ms: decoded.decoded_duration_ms,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedVoice {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
    pub decoded_duration_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceReceipt {
    pub encoding: VoiceEncoding,
    pub sample_rate: u32,
    pub encoded_duration_ms: u32,
    pub encoded_bytes: usize,
    pub transfer_mode: PayloadMode,
    pub decoded_duration_ms: u32,
}

fn facts(info: outrider::voice::ClipInfo) -> VoiceClipFacts {
    VoiceClipFacts {
        encoding: VoiceEncoding::from_codec(info.codec),
        sample_rate: info.sample_rate,
        duration_ms: info.duration_ms,
        encoded_bytes: info.bytes,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceClipError {
    #[error("could not read PCM fixture: {0}")]
    Read(std::io::Error),
    #[error("PCM file is {bytes} bytes, over the {limit} byte limit")]
    PcmTooLarge { bytes: usize, limit: usize },
    #[error("PCM byte length {0} is not whole signed 16-bit samples")]
    OddPcmBytes(usize),
    #[error("PCM byte limit {0} is not aligned to signed 16-bit samples")]
    OddPcmLimit(usize),
    #[error("Pipit could not encode the PCM")]
    Encode,
    #[error("a voice drop cannot contain zero PCM samples")]
    EmptyPcm,
    #[error("this Signalman build cannot encode Pipit codec {0}")]
    UnsupportedEncoding(u8),
    #[error("Pipit could not decode the clip")]
    Decode,
    #[error("invalid voice clip: {0}")]
    Clip(outrider::voice::VoiceError),
    #[error("voice clip bytes disagree with their retained facts")]
    FactMismatch,
    #[error("message has no LXMF audio field")]
    MissingAudioField,
    #[error("LXMF audio mode {0} is not AM_CUSTOM")]
    UnsupportedAudioMode(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_facts_survive_field_seven_and_decode() {
        let pcm = (0..8_000)
            .map(|i| if i % 40 < 20 { 4_000 } else { -4_000 })
            .collect::<Vec<_>>();
        let clip = VoiceClip::encode_pcm(&pcm, VoiceEncoding::Lpc10).unwrap();
        let mut payload = LxmfPayload::text(1.0, b"voice", b"");
        clip.attach(&mut payload).unwrap();
        let recovered = VoiceClip::from_payload(&payload).unwrap();
        let decoded = recovered.decode().unwrap();

        assert_eq!(recovered, clip);
        assert_eq!(clip.facts().sample_rate, 8_000);
        assert_eq!(clip.facts().duration_ms, 1_000);
        assert_eq!(decoded.sample_rate, 8_000);
        assert_eq!(decoded.decoded_duration_ms, 1_000);
        assert_eq!(decoded.pcm.len(), pcm.len());
    }

    #[test]
    fn empty_pcm_is_refused_before_encoding() {
        assert!(matches!(
            VoiceClip::encode_pcm(&[], VoiceEncoding::Lpc10),
            Err(VoiceClipError::EmptyPcm)
        ));
    }
}
