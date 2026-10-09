//! LXMF's registry of field numbers and the specifiers that go with them (`LXMF.py` 8-143).
//!
//! Supporting any of these is optional, both sending and receiving. Unallocated numbers up
//! to 0x80 are reserved; experimental fields belong above 0xFF.

pub const EMBEDDED_LXMS: u8 = 0x01;
pub const TELEMETRY: u8 = 0x02;
pub const TELEMETRY_STREAM: u8 = 0x03;
pub const ICON_APPEARANCE: u8 = 0x04;
pub const FILE_ATTACHMENTS: u8 = 0x05;
pub const IMAGE: u8 = 0x06;
pub const AUDIO: u8 = 0x07;
/// Bytes: the full thread id hash.
pub const THREAD: u8 = 0x08;
pub const COMMANDS: u8 = 0x09;
pub const RESULTS: u8 = 0x0A;
pub const GROUP: u8 = 0x0B;
pub const TICKET: u8 = 0x0C;
pub const EVENT: u8 = 0x0D;
pub const RNR_REFS: u8 = 0x0E;
/// One of the [`renderer`] values.
pub const RENDERER: u8 = 0x0F;
/// Bytes: the full hash of the message replied to.
pub const REPLY_TO: u8 = 0x30;
/// Bytes: the quoted content, UTF-8.
pub const REPLY_QUOTE: u8 = 0x31;
/// A map keyed by [`reaction`].
pub const REACTION: u8 = 0x40;
/// A map keyed by [`comment`]; the comment itself is the message content.
pub const COMMENT: u8 = 0x41;
/// A map keyed by [`continuation`]; the continuation itself is the message content.
pub const CONTINUATION: u8 = 0x42;
/// Bridged or embedded data: a type identifier, the data, and metadata.
pub const CUSTOM_TYPE: u8 = 0xFB;
pub const CUSTOM_DATA: u8 = 0xFC;
pub const CUSTOM_META: u8 = 0xFD;
/// For development, testing and debugging.
pub const NON_SPECIFIC: u8 = 0xFE;
pub const DEBUG: u8 = 0xFF;

/// Audio modes for the [`AUDIO`](crate::fields::AUDIO) field.
pub mod audio {
    pub const CODEC2_450PWB: u8 = 0x01;
    pub const CODEC2_450: u8 = 0x02;
    pub const CODEC2_700C: u8 = 0x03;
    pub const CODEC2_1200: u8 = 0x04;
    pub const CODEC2_1300: u8 = 0x05;
    pub const CODEC2_1400: u8 = 0x06;
    pub const CODEC2_1600: u8 = 0x07;
    pub const CODEC2_2400: u8 = 0x08;
    pub const CODEC2_3200: u8 = 0x09;
    pub const OPUS_OGG: u8 = 0x10;
    pub const OPUS_LBW: u8 = 0x11;
    pub const OPUS_MBW: u8 = 0x12;
    pub const OPUS_PTT: u8 = 0x13;
    pub const OPUS_RT_HDX: u8 = 0x14;
    pub const OPUS_RT_FDX: u8 = 0x15;
    pub const OPUS_STANDARD: u8 = 0x16;
    pub const OPUS_HQ: u8 = 0x17;
    pub const OPUS_BROADCAST: u8 = 0x18;
    pub const OPUS_LOSSLESS: u8 = 0x19;
    /// Unspecified: the receiver works the format out from the data.
    pub const CUSTOM: u8 = 0xFF;
}

/// How a receiver should render the content, for the [`RENDERER`](crate::fields::RENDERER) field.
pub mod renderer {
    pub const PLAIN: u8 = 0x00;
    pub const MICRON: u8 = 0x01;
    pub const MARKDOWN: u8 = 0x02;
    pub const BBCODE: u8 = 0x03;
}

/// Keys of the [`REACTION`](crate::fields::REACTION) map.
pub mod reaction {
    /// Bytes: the full hash of the message reacted to.
    pub const TO: u8 = 0x00;
    /// Bytes: the reaction, UTF-8.
    pub const CONTENT: u8 = 0x01;
}

/// Keys of the [`COMMENT`](crate::fields::COMMENT) map.
pub mod comment {
    /// Bytes: the full hash of the message commented on.
    pub const FOR: u8 = 0x00;
}

/// Keys of the [`CONTINUATION`](crate::fields::CONTINUATION) map.
pub mod continuation {
    /// Bytes: the full hash of the message continued.
    pub const OF: u8 = 0x00;
}

/// Keys of a propagation node announce's metadata map.
pub mod pn_meta {
    pub const VERSION: u8 = 0x00;
    pub const NAME: u8 = 0x01;
    pub const SYNC_STRATUM: u8 = 0x02;
    pub const SYNC_THROTTLE: u8 = 0x03;
    pub const AUTH_BAND: u8 = 0x04;
    pub const UTIL_PRESSURE: u8 = 0x05;
    pub const IMPL_NAME: u8 = 0xFE;
    pub const CUSTOM: u8 = 0xFF;
}

/// Feature codes a delivery announce declares support for.
pub const SF_COMPRESSION: u8 = 0x00;

#[cfg(test)]
mod tests {
    use super::*;

    /// Numbers observed on the wire by `oracle/capture_fields.py` (audio, the 1.1.1 additions),
    /// and no two message fields sharing a number.
    #[test]
    fn the_registry_matches_capture_and_is_unambiguous() {
        assert_eq!(AUDIO, 7);
        assert_eq!(audio::CUSTOM, 255);
        assert_eq!(
            [REPLY_TO, REPLY_QUOTE, REACTION, COMMENT, CONTINUATION],
            [48, 49, 64, 65, 66]
        );
        let all = [
            EMBEDDED_LXMS,
            TELEMETRY,
            TELEMETRY_STREAM,
            ICON_APPEARANCE,
            FILE_ATTACHMENTS,
            IMAGE,
            AUDIO,
            THREAD,
            COMMANDS,
            RESULTS,
            GROUP,
            TICKET,
            EVENT,
            RNR_REFS,
            RENDERER,
            REPLY_TO,
            REPLY_QUOTE,
            REACTION,
            COMMENT,
            CONTINUATION,
            CUSTOM_TYPE,
            CUSTOM_DATA,
            CUSTOM_META,
            NON_SPECIFIC,
            DEBUG,
        ];
        let mut sorted = all;
        sorted.sort_unstable();
        assert!(sorted.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
