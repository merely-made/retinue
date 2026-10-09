//! The RNS 1.3.8 wire layouts: the channel [`Envelope`] and the `Buffer` [`StreamFrame`].

use alloc::vec::Vec;

/// The sequence space: sequences are 16-bit and wrap at this modulus (RNS
/// `SEQ_MODULUS`). Comparisons use wrapping distance with a half-modulus split to
/// tell "ahead" (a future packet to buffer) from "behind" (an old duplicate).
pub const SEQ_MODULUS: u32 = 65536;

/// RNS `Buffer`'s stream-frame message type: a stream chunk rides a
/// [`Channel`](super::Channel) envelope under this msgtype (RNS `StreamDataMessage.MSGTYPE`,
/// `buffer_wire.json`).
pub const STREAM_MSGTYPE: u16 = 0xFF00;

/// The largest stream id. The id is the low 14 bits of the [`StreamFrame`] header (RNS
/// `StreamDataMessage.STREAM_ID_MAX`); the top two bits are the eof / compressed flags.
pub const STREAM_ID_MAX: u16 = 0x3FFF;

/// The most stream data bytes in one [`StreamFrame`] (RNS `StreamDataMessage.MAX_DATA_LEN`):
/// the link MDU less the 6-byte envelope header and the 2-byte stream header (`OVERHEAD` 8).
pub const MAX_DATA_LEN: usize = 423;

/// One channel message on the wire: `[msgtype u16][sequence u16][length u16][payload]`,
/// big-endian. This is RNS 1.3.8's `Channel.Envelope` layout exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    /// Registered message type (identifies the message class on the wire).
    pub msgtype: u16,
    /// Windowed 16-bit sequence number.
    pub sequence: u16,
    /// Message payload.
    pub payload: Vec<u8>,
}

impl Envelope {
    /// Encode to the RNS wire layout.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(6 + self.payload.len());
        out.extend_from_slice(&self.msgtype.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Decode from the RNS wire layout, or `None` if malformed / the declared length
    /// does not match.
    pub fn decode(bytes: &[u8]) -> Option<Envelope> {
        let msgtype = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?);
        let sequence = u16::from_be_bytes(bytes.get(2..4)?.try_into().ok()?);
        let length = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
        let payload = bytes.get(6..6 + length)?.to_vec();
        Some(Envelope {
            msgtype,
            sequence,
            payload,
        })
    }
}

/// One RNS `Buffer` stream frame, the payload of a [`Channel`](super::Channel) envelope:
/// `[u16 BE header][data]`, header = `eof<<15 | compressed<<14 | stream_id`. The envelope's
/// length field implies the data length. RNS 1.3.8 `StreamDataMessage.pack()` exactly
/// (`buffer_wire.json`).
///
/// `compressed` marks a bz2 transform of `data` before framing, not a layout change.
/// Retinue never sets it on send; [`Buffer`](super::Buffer) decodes it on receive with the
/// `compression` feature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamFrame {
    /// Stream id (14-bit): which multiplexed stream this chunk belongs to.
    pub stream_id: u16,
    /// End-of-stream marker: the last frame of this stream.
    pub eof: bool,
    /// Whether `data` is bz2-compressed (see the type docs).
    pub compressed: bool,
    /// The stream bytes (uncompressed unless `compressed`).
    pub data: Vec<u8>,
}

impl StreamFrame {
    const EOF_BIT: u16 = 0x8000;
    const COMPRESSED_BIT: u16 = 0x4000;

    /// Encode to the RNS stream-frame layout.
    pub fn encode(&self) -> Vec<u8> {
        let mut header = self.stream_id & STREAM_ID_MAX;
        if self.eof {
            header |= Self::EOF_BIT;
        }
        if self.compressed {
            header |= Self::COMPRESSED_BIT;
        }
        let mut out = Vec::with_capacity(2 + self.data.len());
        out.extend_from_slice(&header.to_be_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Decode from the RNS stream-frame layout, or `None` if shorter than the header.
    pub fn decode(bytes: &[u8]) -> Option<StreamFrame> {
        let header = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?);
        Some(StreamFrame {
            stream_id: header & STREAM_ID_MAX,
            eof: header & Self::EOF_BIT != 0,
            compressed: header & Self::COMPRESSED_BIT != 0,
            data: bytes.get(2..)?.to_vec(),
        })
    }
}
