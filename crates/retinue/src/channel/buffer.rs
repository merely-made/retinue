//! [`Buffer`]: an RNS `Buffer`-compatible byte stream over a [`Channel`].

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use super::{
    Channel, ChannelError, Envelope, MAX_DATA_LEN, REORDER_MAX, STREAM_ID_MAX, STREAM_MSGTYPE,
    StreamFrame,
};

/// Default per-frame chunk: RNS's own `MAX_DATA_LEN` — the most stream bytes that fit in
/// one link data packet after the envelope and stream headers.
pub const DEFAULT_CHUNK: usize = MAX_DATA_LEN;

/// Default maximum decoded bytes in one compressed stream frame. This is separate
/// from `READ_BYTES`, which only bounds the ready-to-read queue.
pub const DEFAULT_DECODED_FRAME_LIMIT: usize = 65_536;

/// A terminal stream receive failure. Once set, the buffer delivers only bytes
/// decoded before the bad frame and never reports a clean receive EOF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDecodeError {
    /// This build has no bz2 decoder.
    UnsupportedCompression,
    /// The bz2 data is malformed.
    InvalidCompression,
    /// Decoded bytes exceeded the configured per-frame output ceiling.
    DecodedFrameLimitExceeded { limit: usize },
}

/// Invalid decoded-frame configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDecodeLimitError {
    /// The limit must be positive and leave room for one sentinel byte.
    InvalidLimit,
}

/// A byte stream over a reliable [`Channel`], RNS `Buffer`-wire-compatible. Each write
/// chunk is a [`StreamFrame`] in a [`STREAM_MSGTYPE`] envelope; [`read`](Self::read)
/// concatenates delivered frames in order. Sans-io: a driver pumps
/// [`poll_transmit`](Self::poll_transmit) / [`handle`](Self::handle) /
/// [`on_proof`](Self::on_proof) against the wire.
///
/// A buffer sends on `send_stream_id` and reads only `recv_stream_id`, so one channel can
/// carry several streams. [`finish`](Self::finish) sends eof;
/// [`recv_finished`](Self::recv_finished) reports the peer's.
///
/// Other message types are sequenced and proved, so the stream does not stall, then
/// dropped; a caller that wants them uses [`Channel::recv_message`] directly.
///
/// Retinue does not compress sent frames. With the `compression` feature, received
/// compressed frames are decoded; failure is terminal, reported by
/// [`receive_error`](Self::receive_error), and never a healthy EOF.
pub struct Buffer<
    const WINDOW: usize = 64,
    const QUEUE: usize = 256,
    const REORDER: usize = REORDER_MAX,
    const READ_BYTES: usize = 65_536,
> {
    channel: Channel<WINDOW, QUEUE, REORDER>,
    max_chunk: usize,
    send_stream_id: u16,
    recv_stream_id: u16,
    /// Bytes decoded from delivered frames, awaiting the reader.
    ///
    /// Heap-backed and bounded at runtime by `READ_BYTES`: a `Deque<u8, READ_BYTES>` would
    /// commit that much static storage per idle link. A decoded frame that does not fit
    /// waits in `pending_frame`; beyond that, pressure reaches the channel's bounded inbox.
    pub(super) read_buf: VecDeque<u8>,
    /// Remaining bytes from one delivered frame. Decompression still allocates the full
    /// decoded frame; `READ_BYTES` bounds the ready-to-read queue, not that allocation.
    pending_frame: Option<(Vec<u8>, usize)>,
    recv_eof: bool,
    decoded_frame_limit: usize,
    receive_error: Option<StreamDecodeError>,
}

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize, const READ_BYTES: usize> Default
    for Buffer<WINDOW, QUEUE, REORDER, READ_BYTES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize, const READ_BYTES: usize>
    Buffer<WINDOW, QUEUE, REORDER, READ_BYTES>
{
    /// A buffer with the default channel and chunk size, stream id 0 both ways.
    pub fn new() -> Self {
        Self::with_channel(Channel::new(STREAM_MSGTYPE), DEFAULT_CHUNK)
    }

    /// A default dynamic channel with an explicit application chunk ceiling.
    pub fn with_max_chunk(max_chunk: usize) -> Self {
        Self::with_channel(Channel::new(STREAM_MSGTYPE), max_chunk)
    }

    /// A buffer whose channel starts with a medium-specific RTT estimate.
    pub fn with_initial_rtt(initial_rtt: u64) -> Self {
        Self::with_channel(
            Channel::with_initial_rtt(STREAM_MSGTYPE, initial_rtt),
            DEFAULT_CHUNK,
        )
    }

    /// A buffer whose dynamic channel is capped for the selected medium.
    pub fn with_initial_rtt_and_max_window(initial_rtt: u64, max_window: u32) -> Self {
        Self::with_policy(initial_rtt, max_window, DEFAULT_CHUNK)
    }

    /// A buffer with explicit RTT, dynamic-window ceiling, and application chunk size.
    pub fn with_policy(initial_rtt: u64, max_window: u32, max_chunk: usize) -> Self {
        Self::with_channel(
            Channel::with_initial_rtt_and_max_window(STREAM_MSGTYPE, initial_rtt, max_window),
            max_chunk,
        )
    }

    /// A buffer over an explicit channel and chunk size, stream id 0 both ways.
    pub fn with_channel(channel: Channel<WINDOW, QUEUE, REORDER>, max_chunk: usize) -> Self {
        Self::with_streams(channel, max_chunk, 0, 0)
    }

    /// A buffer with explicit send / receive stream ids (each clamped to
    /// [`STREAM_ID_MAX`]) — one channel multiplexing distinct streams.
    pub fn with_streams(
        channel: Channel<WINDOW, QUEUE, REORDER>,
        max_chunk: usize,
        send_stream_id: u16,
        recv_stream_id: u16,
    ) -> Self {
        Self {
            channel,
            max_chunk: max_chunk.clamp(1, MAX_DATA_LEN),
            send_stream_id: send_stream_id & STREAM_ID_MAX,
            recv_stream_id: recv_stream_id & STREAM_ID_MAX,
            read_buf: VecDeque::new(),
            pending_frame: None,
            recv_eof: false,
            decoded_frame_limit: DEFAULT_DECODED_FRAME_LIMIT,
            receive_error: None,
        }
    }

    /// Set the decoded output ceiling for each compressed frame. Uncompressed
    /// frames are already bounded by the link packet and retain their prior behavior.
    /// `limit + 1` bytes are reserved for output and oversize detection; bz2's
    /// decoder workspace is additional. Configure before receiving packets.
    pub fn set_decoded_frame_limit(&mut self, limit: usize) -> Result<(), StreamDecodeLimitError> {
        if limit == 0 || limit >= isize::MAX as usize {
            return Err(StreamDecodeLimitError::InvalidLimit);
        }
        self.decoded_frame_limit = limit;
        Ok(())
    }

    /// The active per-frame decoded output ceiling.
    pub fn decoded_frame_limit(&self) -> usize {
        self.decoded_frame_limit
    }

    /// Queue bytes for reliable, in-order delivery, chunked into [`StreamFrame`]s.
    ///
    /// Returns how many bytes were accepted, which is fewer than `bytes.len()` when the
    /// send queue fills. A caller writing faster than the link drains is told so, rather
    /// than growing the queue without limit. Chunking means the split is always on a frame
    /// boundary, so a partial accept never tears a frame.
    #[must_use]
    pub fn write(&mut self, bytes: &[u8]) -> usize {
        let mut accepted = 0;
        for chunk in bytes.chunks(self.max_chunk) {
            if self.send_frame(chunk.to_vec(), false).is_err() {
                break;
            }
            accepted += chunk.len();
        }
        accepted
    }

    /// Mark the send stream finished: queue an empty eof frame. RNS also accepts eof
    /// riding a final data frame; a standalone eof is the simpler equivalent.
    ///
    /// Returns whether the eof frame was queued; a full send queue refuses it, and the
    /// caller retries once [`poll_transmit`](Self::poll_transmit) has drained room.
    pub fn finish(&mut self) -> bool {
        self.send_frame(Vec::new(), true).is_ok()
    }

    fn send_frame(&mut self, data: Vec<u8>, eof: bool) -> Result<(), ()> {
        let frame = StreamFrame {
            stream_id: self.send_stream_id,
            eof,
            compressed: false,
            data,
        };
        self.channel.send(frame.encode()).map_err(|_| ())
    }

    /// Copy up to `out.len()` delivered bytes into `out`, returning the count read.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        self.fill();
        let n = out.len().min(self.read_buf.len());
        for slot in out.iter_mut().take(n) {
            *slot = self.read_buf.pop_front().expect("len checked");
        }
        n
    }

    /// Take up to `READ_BYTES` currently-available delivered bytes.
    pub fn read_available(&mut self) -> Vec<u8> {
        self.fill();
        self.read_buf.drain(..).collect()
    }

    /// Recover a compressed stream frame within the configured output ceiling.
    ///
    /// Without the `compression` feature no bz2 decoder is linked, so these bytes are
    /// unreadable here. RNS compresses only when it shrinks the payload.
    fn decompressed(&self, data: &[u8]) -> Result<Vec<u8>, StreamDecodeError> {
        #[cfg(feature = "compression")]
        {
            crate::resource::decompress_bounded(data, self.decoded_frame_limit).map_err(|e| match e
            {
                crate::resource::BoundedDecompressError::InvalidData => {
                    StreamDecodeError::InvalidCompression
                }
                crate::resource::BoundedDecompressError::LimitExceeded => {
                    StreamDecodeError::DecodedFrameLimitExceeded {
                        limit: self.decoded_frame_limit,
                    }
                }
            })
        }
        #[cfg(not(feature = "compression"))]
        {
            let _ = data;
            Err(StreamDecodeError::UnsupportedCompression)
        }
    }

    fn fill(&mut self) {
        if self.receive_error.is_some() {
            return;
        }
        // At most one decoded frame waits outside read_buf; later frames stay in the
        // channel's bounded inbox.
        while self.read_buf.len() < READ_BYTES {
            if let Some((data, cursor)) = &mut self.pending_frame {
                let count = (READ_BYTES - self.read_buf.len()).min(data.len() - *cursor);
                self.read_buf
                    .extend(data[*cursor..*cursor + count].iter().copied());
                *cursor += count;
                if *cursor == data.len() {
                    self.pending_frame = None;
                }
                continue;
            }
            let Some((msgtype, msg)) = self.channel.recv_message() else {
                break;
            };
            if msgtype != STREAM_MSGTYPE {
                continue; // already sequenced and proved; not stream data
            }
            let Some(frame) = StreamFrame::decode(&msg) else {
                continue; // malformed frame; the channel already ordered/deduped it
            };
            if frame.stream_id != self.recv_stream_id {
                continue; // a different multiplexed stream on the same channel
            }
            let data = if frame.compressed {
                // Already proved to the peer, so dropping it would be silent loss.
                match self.decompressed(&frame.data) {
                    Ok(data) => Some(data),
                    Err(error) => {
                        self.receive_error = Some(error);
                        break;
                    }
                }
            } else {
                Some(frame.data)
            };
            if let Some(data) = data.filter(|data| !data.is_empty()) {
                self.pending_frame = Some((data, 0));
            }
            if frame.eof {
                self.recv_eof = true;
            }
        }
    }

    /// Whether the peer's eof frame has arrived and all earlier bytes have been read.
    pub fn recv_finished(&mut self) -> bool {
        self.fill();
        self.receive_error.is_none()
            && self.recv_eof
            && self.read_buf.is_empty()
            && self.pending_frame.is_none()
    }

    /// The sticky terminal receive error, if any. Calling this decodes ready frames
    /// up to the read buffer bound. A packet may have been proved at Channel admission
    /// before an earlier queued frame reaches this decoder; callers must treat this
    /// error as a failed link and must not present a healthy EOF.
    pub fn receive_error(&mut self) -> Option<StreamDecodeError> {
        self.fill();
        self.receive_error
    }

    /// Compatibility flag for callers using the previous diagnostic API.
    pub fn had_unsupported_frame(&self) -> bool {
        self.receive_error.is_some()
    }

    /// Envelopes to put on the wire now — see [`Channel::poll_transmit`].
    pub fn poll_transmit(&mut self, now: u64) -> Vec<Envelope> {
        self.channel.poll_transmit(now)
    }

    /// Feed a received envelope in — see [`Channel::handle`]. Returns whether the driver
    /// should prove the packet (`false` when the reorder buffer is full).
    #[must_use]
    pub fn handle(&mut self, envelope: Envelope) -> bool {
        if self.receive_error.is_some() {
            return false;
        }
        self.channel.handle(envelope)
    }

    /// Release a proven sequence — see [`Channel::on_proof`].
    pub fn on_proof(&mut self, sequence: u16, now: u64) {
        self.channel.on_proof(sequence, now);
    }

    /// The current send window — see [`Channel::window`].
    pub fn window(&self) -> u32 {
        self.channel.window()
    }

    /// The current RTT estimate — see [`Channel::rtt`].
    pub fn rtt(&self) -> u64 {
        self.channel.rtt()
    }

    /// Replace the initial RTT estimate — see [`Channel::set_initial_rtt`].
    pub fn set_initial_rtt(&mut self, rtt: u64) {
        self.channel.set_initial_rtt(rtt);
    }

    /// Set the per-envelope try limit — see [`Channel::set_max_tries`].
    pub fn set_max_tries(&mut self, tries: u8) {
        self.channel.set_max_tries(tries);
    }

    /// Why the underlying channel stopped, once it has — see [`Channel::error`].
    pub fn channel_error(&self) -> Option<ChannelError> {
        self.channel.error()
    }

    /// Whether everything written has been sent and proven.
    pub fn send_idle(&self) -> bool {
        self.channel.send_idle()
    }
}
