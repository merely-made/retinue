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
//! # Why there is no field constant here
//!
//! LXMF carries its fields in a MessagePack map keyed by number, and the
//! stock ecosystem has assigned numbers for telemetry, images, audio and so
//! on. Outrider takes a field number **only from a capture or from public
//! prose**, per the v1 scope in
//! `design_docs/2026-07-25_outrider_lxmf_founding.md`, and the LXMF audio
//! field's number is documented in neither: the project README states that
//! full protocol documentation is still planned, and the audio mode list is
//! published only as source, which this crate does not read.
//!
//! So [`FieldKey`] is supplied by the caller and no default is offered.
//! Choosing one is a protocol decision, not an implementation detail, and
//! guessing it wrong is worse than leaving it open: a stock client that
//! recognised the number would hand a Pipit clip to a decoder expecting a
//! different codec entirely and render noise.
//!
//! [`find_clip`] exists for the same reason. Because a clip is
//! self-describing, a receiver can locate one without either side having
//! agreed a number at all.

use pipit::{ClipHeader, Codec};
use rmpv::Value;

use crate::codec::LxmfPayload;

/// Refuse a clip larger than this unless the caller says otherwise. Ten
/// seconds of vocoded speech is about 3 KB, so this is generous for a voice
/// message while still bounding what a stranger can make us hold.
pub const DEFAULT_MAX_CLIP_BYTES: usize = 256 * 1024;

/// The MessagePack field number a clip travels under.
///
/// Deliberately not a constant. See the module documentation: the number is
/// the caller's protocol decision until a capture settles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldKey(pub u64);

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
    // part of the clip and has no business travelling.
    let carried = Value::Binary(clip[..info.bytes].to_vec());
    match entries.iter_mut().find(|(k, _)| key_of(k) == Some(key)) {
        Some((_, slot)) => *slot = carried,
        None => entries.push((Value::from(key.0), carried)),
    }
    Ok(info)
}

/// The clip at `key`, if there is a well-formed one there.
pub fn clip_at(payload: &LxmfPayload, key: FieldKey) -> Option<(&[u8], ClipInfo)> {
    let Value::Map(entries) = &payload.fields else {
        return None;
    };
    let (_, value) = entries.iter().find(|(k, _)| key_of(k) == Some(key))?;
    let bytes = value.as_slice()?;
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
        let bytes = v.as_slice()?;
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
        assert!((energy / pcm.len() as f64).sqrt() > 100.0, "silence came back");
    }

    #[test]
    fn replacing_a_clip_does_not_duplicate_the_field() {
        let mut payload = payload();
        attach(&mut payload, FieldKey(7), &clip()).unwrap();
        attach(&mut payload, FieldKey(7), &clip()).unwrap();
        let Value::Map(entries) = &payload.fields else {
            unreachable!()
        };
        assert_eq!(entries.iter().filter(|(k, _)| key_of(k) == Some(FieldKey(7))).count(), 1);
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
            Err(VoiceError::TooLarge { bytes: 333, limit: 100 })
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
