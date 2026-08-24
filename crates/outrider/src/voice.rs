//! Voice clips as LXMF message fields.
//!
//! A recorded voice message is a [Pipit](https://crates.io/crates/pipit)
//! clip: a self-describing block of encoded speech that names its own codec,
//! sample rate and duration. This module carries one in an LXMF message and
//! takes it back out again, checking at the boundary that what arrived is
//! actually a clip and is intact.
//!
//! The split is deliberate. The clip is Pipit's, so a host with no radio
//! stack can decode a voice message; the envelope is Outrider's, and no
//! sender, recipient, timestamp or signature ever enters a clip.
//!
//! # Which field, and why the mode matters
//!
//! LXMF carries its fields in a MessagePack map keyed by number. Those
//! numbers are not in any public prose, so they were taken the way
//! outrider's v1 scope requires, from a capture: see
//! `oracle/capture_fields.py`, which reads the stock client's public
//! constants at runtime and then confirms the number on the wire by packing
//! a real message. Audio is field 7, and its value is a two-element list of
//! an audio mode and the encoded bytes.
//!
//! The mode is the part that decides whether riding that field is honest.
//! The stock list is Codec2 and Opus, and Pipit is neither, so claiming one
//! of those would hand a decoder something it would render as noise. But the
//! same capture found **`AM_CUSTOM`**, a mode that means exactly "audio in a
//! codec outside this list". That is what a Pipit clip is, so [`attach`]
//! writes it: the message reads as voice to any client, no client is invited
//! to misdecode it, and the clip's own header says which codec it actually
//! is.
//!
//! [`find_clip`] still locates a clip by its own header rather than by field
//! number, so two Retinue peers interoperate even if they never agree on a
//! field at all.

use pipit::{ClipHeader, Codec};
use rmpv::Value;

use crate::codec::LxmfPayload;

/// Refuse a clip larger than this unless the caller says otherwise. Ten
/// seconds of vocoded speech is about 3 KB, so this is generous for a voice
/// message while still bounding what a stranger can make us hold.
pub const DEFAULT_MAX_CLIP_BYTES: usize = 256 * 1024;

/// The MessagePack field number a clip travels under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldKey(pub u64);

impl FieldKey {
    /// The field stock LXMF carries audio in, observed as 7 by
    /// `oracle/capture_fields.py` against LXMF 0.9.6 and re-confirmed against 1.1.1,
    /// which added five fields (48, 49, 64, 65, 66) without moving any existing one.
    pub const AUDIO: Self = Self(7);
}

/// The audio mode meaning "a codec outside the standard list", observed as
/// 255 in the same capture.
///
/// Pipit clips are sent under this rather than under a Codec2 or Opus mode,
/// because they are not those and a client should not try to decode them as
/// though they were.
pub const AM_CUSTOM: u8 = 255;

/// What a clip says about itself, read from its header without decoding any
/// audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipInfo {
    pub codec: Codec,
    pub sample_rate: u32,
    pub duration_ms: u32,
    /// Length of the clip on the wire, header included.
    pub bytes: usize,
}

/// Why a clip could not be carried or recovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VoiceError {
    /// The bytes are not a Pipit clip at all.
    #[error("not a pipit clip")]
    NotAClip,
    /// A clip header that does not describe the bytes that followed it.
    #[error("clip header declares {declared} bytes but {actual} are present")]
    Truncated { declared: usize, actual: usize },
    /// Larger than the caller allowed.
    #[error("clip is {bytes} bytes, over the {limit} byte limit")]
    TooLarge { bytes: usize, limit: usize },
    /// The payload's fields are not a MessagePack map, so nothing can be
    /// attached to them.
    #[error("message fields are not a map")]
    FieldsNotAMap,
}

/// Read a clip's header without decoding it.
///
/// This is the boundary check: it proves the bytes are a clip, that the
/// header is one this build understands, and that the clip is not truncated.
pub fn describe(clip: &[u8]) -> Result<ClipInfo, VoiceError> {
    let header = ClipHeader::parse(clip).map_err(|_| VoiceError::NotAClip)?;
    let declared = header.encoded_len();
    if clip.len() < declared {
        return Err(VoiceError::Truncated {
            declared,
            actual: clip.len(),
        });
    }
    Ok(ClipInfo {
        codec: header.codec,
        sample_rate: header.sample_rate,
        duration_ms: header.duration_ms(),
        bytes: declared,
    })
}

/// Attach a clip to a message under `key`, replacing anything already there.
///
/// The clip is checked before it is attached, so a malformed one never
/// reaches the wire.
pub fn attach(
    payload: &mut LxmfPayload,
    key: FieldKey,
    clip: &[u8],
) -> Result<ClipInfo, VoiceError> {
    attach_bounded(payload, key, clip, DEFAULT_MAX_CLIP_BYTES)
}

/// [`attach`] with an explicit size ceiling.
pub fn attach_bounded(
    payload: &mut LxmfPayload,
    key: FieldKey,
    clip: &[u8],
    max_clip_bytes: usize,
) -> Result<ClipInfo, VoiceError> {
    let info = describe(clip)?;
    if info.bytes > max_clip_bytes {
        return Err(VoiceError::TooLarge {
            bytes: info.bytes,
            limit: max_clip_bytes,
        });
    }
    let Value::Map(entries) = &mut payload.fields else {
        return Err(VoiceError::FieldsNotAMap);
    };
    // Carry exactly the bytes the header accounts for: trailing slack is not
    // part of the clip and has no business travelling. The two-element
    // [mode, bytes] shape is what stock LXMF puts in an audio field.
    let carried = Value::Array(vec![
        Value::from(AM_CUSTOM),
        Value::Binary(clip[..info.bytes].to_vec()),
    ]);
    match entries.iter_mut().find(|(k, _)| key_of(k) == Some(key)) {
        Some((_, slot)) => *slot = carried,
        None => entries.push((Value::from(key.0), carried)),
    }
    Ok(info)
}

/// The audio at `key`, whatever codec it is in.
///
/// Returns the declared mode and the raw bytes, so a caller can tell a
/// Codec2 message it cannot decode from one of ours rather than showing
/// nothing at all. Accepts the stock two-element shape and a bare binary
/// alike; the latter is not what this module writes, but a reader has no
/// reason to be strict about it.
pub fn audio_at(payload: &LxmfPayload, key: FieldKey) -> Option<(u8, &[u8])> {
    let Value::Map(entries) = &payload.fields else {
        return None;
    };
    let (_, value) = entries.iter().find(|(k, _)| key_of(k) == Some(key))?;
    audio_value(value)
}

/// The stock `[mode, bytes]` pair, or a bare binary with no mode declared.
fn audio_value(value: &Value) -> Option<(u8, &[u8])> {
    match value {
        Value::Array(items) if items.len() == 2 => {
            let mode = u8::try_from(items[0].as_u64()?).ok()?;
            Some((mode, items[1].as_slice()?))
        }
        other => Some((AM_CUSTOM, other.as_slice()?)),
    }
}

/// The clip at `key`, if there is a well-formed one there.
pub fn clip_at(payload: &LxmfPayload, key: FieldKey) -> Option<(&[u8], ClipInfo)> {
    let (_, bytes) = audio_at(payload, key)?;
    let info = describe(bytes).ok()?;
    Some((&bytes[..info.bytes], info))
}

/// The first well-formed clip in the message, whatever number it arrived
/// under.
///
/// A clip identifies itself, so this works without the two sides having
/// agreed a field number. Fields that are not clips are passed over rather
/// than refused: an LXMF message may legitimately carry telemetry, an image,
/// or anything else beside the voice.
pub fn find_clip(payload: &LxmfPayload) -> Option<(FieldKey, &[u8], ClipInfo)> {
    let Value::Map(entries) = &payload.fields else {
        return None;
    };
    entries.iter().find_map(|(k, v)| {
        let key = key_of(k)?;
        let (_, bytes) = audio_value(v)?;
        let info = describe(bytes).ok()?;
        Some((key, &bytes[..info.bytes], info))
    })
}

/// Remove the clip at `key`, returning whether one was there.
pub fn detach(payload: &mut LxmfPayload, key: FieldKey) -> bool {
    let Value::Map(entries) = &mut payload.fields else {
        return false;
    };
    let before = entries.len();
    entries.retain(|(k, _)| key_of(k) != Some(key));
    entries.len() != before
}

/// LXMF field keys are numbers; anything else is not one of ours.
fn key_of(value: &Value) -> Option<FieldKey> {
    value.as_u64().map(FieldKey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode, prepare};

    /// A short spoken-sounding clip through the vocoder, which is the shape a
    /// LoRa voice drop actually takes.
    fn clip() -> Vec<u8> {
        let pcm: Vec<i16> = (0..8_000)
            .map(|i| {
                let t = i as f32 / 8000.0;
                let v = (t * 180.0 * core::f32::consts::TAU).sin() * 0.6
                    + (t * 400.0 * core::f32::consts::TAU).sin() * 0.2;
                (v * 8000.0) as i16
            })
            .collect();
        pipit::encode_clip(&pcm, pipit::ClipParams::lpc10()).unwrap()
    }

    fn payload() -> LxmfPayload {
        LxmfPayload::text(1_753_603_200.5, b"voice", b"")
    }

    #[test]
    fn a_clip_describes_itself_without_being_decoded() {
        let info = describe(&clip()).unwrap();
        assert_eq!(info.codec, pipit::Codec::Lpc10);
        assert_eq!(info.sample_rate, 8_000);
        assert_eq!(info.duration_ms, 1_000);
        assert_eq!(info.bytes, 333);
    }

    #[test]
    fn attach_and_recover_by_key() {
        let clip = clip();
        let mut payload = payload();
        let info = attach(&mut payload, FieldKey(7), &clip).unwrap();

        let (recovered, recovered_info) = clip_at(&payload, FieldKey(7)).unwrap();
        assert_eq!(recovered, &clip[..]);
        assert_eq!(recovered_info, info);
        assert!(clip_at(&payload, FieldKey(8)).is_none());
    }

    #[test]
    fn a_clip_is_found_without_agreeing_a_field_number() {
        // The receiving side never learns which number the sender chose.
        let clip = clip();
        let mut payload = payload();
        attach(&mut payload, FieldKey(1234), &clip).unwrap();

        let (key, found, info) = find_clip(&payload).unwrap();
        assert_eq!(key, FieldKey(1234));
        assert_eq!(found, &clip[..]);
        assert_eq!(info.duration_ms, 1_000);
    }

    #[test]
    fn other_fields_are_stepped_over_not_refused() {
        let clip = clip();
        let mut payload = payload();
        if let Value::Map(entries) = &mut payload.fields {
            entries.push((Value::from(2u64), Value::Binary(b"telemetry".to_vec())));
            entries.push((Value::from(3u64), Value::from("a string")));
        }
        attach(&mut payload, FieldKey(9), &clip).unwrap();

        let (key, _, _) = find_clip(&payload).unwrap();
        assert_eq!(key, FieldKey(9), "the non-clip fields must not match");
    }

    #[test]
    fn a_clip_survives_the_message_codec() {
        // The carriage claim end to end: attach, sign, encode, decode, and
        // the clip comes back byte for byte.
        let clip = clip();
        let mut payload = payload();
        attach(&mut payload, FieldKey(7), &clip).unwrap();

        let prepared = prepare([1; 16], [2; 16], &payload).unwrap();
        let decoded = decode(&prepared.finish([4; 64])).unwrap();

        let (recovered, info) = clip_at(&decoded.payload, FieldKey(7)).unwrap();
        assert_eq!(recovered, &clip[..], "clip must round-trip unchanged");
        assert_eq!(info.duration_ms, 1_000);
    }

    #[test]
    fn a_decoded_clip_still_renders_speech() {
        // Proves the bytes that crossed the wire are audio, not just equal.
        let clip = clip();
        let mut payload = payload();
        attach(&mut payload, FieldKey(7), &clip).unwrap();
        let prepared = prepare([1; 16], [2; 16], &payload).unwrap();
        let decoded = decode(&prepared.finish([4; 64])).unwrap();

        let (bytes, _) = clip_at(&decoded.payload, FieldKey(7)).unwrap();
        let (header, pcm) = pipit::decode_clip(bytes).unwrap();
        assert_eq!(header.sample_rate, 8_000);
        assert_eq!(pcm.len(), 8_000);
        let energy: f64 = pcm.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        assert!(
            (energy / pcm.len() as f64).sqrt() > 100.0,
            "silence came back"
        );
    }

    #[test]
    fn a_half_rate_clip_carries_and_identifies_itself() {
        // Codec ids travel in the clip header, so a new codec needs nothing
        // from this module. Proving that rather than assuming it.
        let pcm: Vec<i16> = (0..8_000)
            .map(|i| {
                let t = i as f32 / 8000.0;
                ((t * 180.0 * core::f32::consts::TAU).sin() * 7000.0) as i16
            })
            .collect();
        let clip = pipit::encode_clip(&pcm, pipit::ClipParams::lpc10_half()).unwrap();

        let mut payload = payload();
        let info = attach(&mut payload, FieldKey(7), &clip).unwrap();
        assert_eq!(info.codec, pipit::Codec::Lpc10Half);
        assert_eq!(info.duration_ms, 1_000);

        let prepared = prepare([1; 16], [2; 16], &payload).unwrap();
        let decoded = decode(&prepared.finish([4; 64])).unwrap();
        let (found_key, bytes, found) = find_clip(&decoded.payload).unwrap();
        assert_eq!(found_key, FieldKey(7));
        assert_eq!(found.codec, pipit::Codec::Lpc10Half);

        let (_, out) = pipit::decode_clip(bytes).unwrap();
        assert_eq!(out.len(), pcm.len());

        // A third of the airtime of the full-rate vocoder, over the air.
        let full = pipit::encode_clip(&pcm, pipit::ClipParams::lpc10()).unwrap();
        assert!(
            clip.len() < full.len() * 7 / 10,
            "{} vs {}",
            clip.len(),
            full.len()
        );
    }

    #[test]
    fn a_clip_travels_in_the_stock_audio_shape() {
        // Field 7 with a two-element [mode, bytes] value, which is what the
        // capture observed stock LXMF writing.
        let clip = clip();
        let mut payload = payload();
        attach(&mut payload, FieldKey::AUDIO, &clip).unwrap();

        let Value::Map(entries) = &payload.fields else {
            unreachable!()
        };
        let (key, value) = entries
            .iter()
            .find(|(k, _)| key_of(k) == Some(FieldKey::AUDIO))
            .unwrap();
        assert_eq!(key.as_u64(), Some(7));
        let Value::Array(items) = value else {
            panic!("audio must be a two-element list, got {value:?}")
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_u64(), Some(AM_CUSTOM as u64));
        assert_eq!(items[1].as_slice(), Some(&clip[..]));
    }

    #[test]
    fn a_stock_codec2_message_is_seen_but_not_claimed() {
        // The case that decides whether riding field 7 is honest. A stock
        // audio message must be recognisable as audio, so a client can say
        // what it is, while never being mistaken for a clip we can decode.
        let mut payload = payload();
        if let Value::Map(entries) = &mut payload.fields {
            // AM_CODEC2_2400 is mode 8 per the capture.
            entries.push((
                Value::from(7u64),
                Value::Array(vec![Value::from(8u8), Value::Binary(vec![0xAB; 64])]),
            ));
        }

        let (mode, bytes) = audio_at(&payload, FieldKey::AUDIO).unwrap();
        assert_eq!(mode, 8, "the codec2 mode is readable");
        assert_eq!(bytes.len(), 64);

        assert!(
            find_clip(&payload).is_none(),
            "a codec we cannot decode must not be taken for one we can"
        );
        assert!(clip_at(&payload, FieldKey::AUDIO).is_none());
    }

    #[test]
    fn a_bare_binary_is_still_readable() {
        // Written by nothing here, but a reader has no reason to refuse it.
        let clip = clip();
        let mut payload = payload();
        if let Value::Map(entries) = &mut payload.fields {
            entries.push((Value::from(7u64), Value::Binary(clip.clone())));
        }
        let (recovered, _) = clip_at(&payload, FieldKey::AUDIO).unwrap();
        assert_eq!(recovered, &clip[..]);
    }

    #[test]
    fn replacing_a_clip_does_not_duplicate_the_field() {
        let mut payload = payload();
        attach(&mut payload, FieldKey(7), &clip()).unwrap();
        attach(&mut payload, FieldKey(7), &clip()).unwrap();
        let Value::Map(entries) = &payload.fields else {
            unreachable!()
        };
        assert_eq!(
            entries
                .iter()
                .filter(|(k, _)| key_of(k) == Some(FieldKey(7)))
                .count(),
            1
        );
        assert!(detach(&mut payload, FieldKey(7)));
        assert!(!detach(&mut payload, FieldKey(7)));
        assert!(find_clip(&payload).is_none());
    }

    #[test]
    fn malformed_clips_never_reach_the_wire() {
        let mut payload = payload();
        assert_eq!(
            attach(&mut payload, FieldKey(7), b"not audio at all"),
            Err(VoiceError::NotAClip)
        );

        let clip = clip();
        let truncated = &clip[..clip.len() - 40];
        assert!(matches!(
            attach(&mut payload, FieldKey(7), truncated),
            Err(VoiceError::Truncated { .. })
        ));

        assert!(matches!(
            attach_bounded(&mut payload, FieldKey(7), &clip, 100),
            Err(VoiceError::TooLarge {
                bytes: 333,
                limit: 100
            })
        ));

        // Nothing partial was left behind by any of the refusals.
        assert!(find_clip(&payload).is_none());
    }

    #[test]
    fn trailing_slack_is_not_carried() {
        let mut clip = clip();
        let real_len = clip.len();
        clip.extend_from_slice(&[0xAB; 64]);
        let mut payload = payload();
        let info = attach(&mut payload, FieldKey(7), &clip).unwrap();
        assert_eq!(info.bytes, real_len);
        let (carried, _) = clip_at(&payload, FieldKey(7)).unwrap();
        assert_eq!(carried.len(), real_len);
    }

    #[test]
    fn a_hostile_field_is_not_mistaken_for_a_clip() {
        let mut payload = payload();
        if let Value::Map(entries) = &mut payload.fields {
            // Right magic, nothing behind it.
            entries.push((Value::from(5u64), Value::Binary(b"PIPIT".to_vec())));
            // Right magic and a header promising far more than it carries.
            let mut lying = b"PIPIT\x01\x02\x00".to_vec();
            lying.extend_from_slice(&8000u32.to_le_bytes());
            lying.extend_from_slice(&180u16.to_le_bytes());
            lying.extend_from_slice(&u32::MAX.to_le_bytes());
            entries.push((Value::from(6u64), Value::Binary(lying)));
        }
        assert!(find_clip(&payload).is_none());
    }
}
