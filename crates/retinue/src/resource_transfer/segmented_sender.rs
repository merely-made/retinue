//! Publishing a resource of any size as RNS segments (`Resource.py` 274-339, 793-835).

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::ops::Range;

use super::sender::outgoing;
use super::{ResourceSender, Timing};
use crate::link::Link;
use crate::packet::Packet;
use crate::resource::{
    DEFAULT_MAX_DECOMPRESSED_SIZE, MAX_SEGMENT_SIZE, RANDOM_HASH_LEN, pack_metadata,
};
use crate::token::IV_LEN;
use crate::{Error, Result};

/// What a resource carries, as its advertisement's `q` field and `u`/`p` flags say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    /// Application data.
    Data,
    /// A request too large for one packet, named by the truncated hash of its packed form
    /// (`Link.py` 503-506).
    Request([u8; 16]),
    /// The response to the named request.
    Response([u8; 16]),
}

/// How many segments a resource of `total_size` bytes (data plus framed metadata) splits
/// into: one per [`MAX_SEGMENT_SIZE`] (`Resource.py` 307).
pub fn segment_count(total_size: usize) -> usize {
    total_size.saturating_sub(1) / MAX_SEGMENT_SIZE + 1
}

/// The fixed part of a publish: everything needed to build any one segment.
struct Plan<D> {
    link: Link,
    /// The data, released once the last segment is built.
    data: Option<D>,
    len: usize,
    /// Metadata framed by [`pack_metadata`]; it leads the first segment.
    metadata: Option<Vec<u8>>,
    kind: ResourceKind,
    compress: bool,
    segments: usize,
    original_hash: [u8; 32],
    /// Every segment's retransmission timing.
    timing: Timing,
}

impl<D: AsRef<[u8]>> Plan<D> {
    fn framed_metadata_len(&self) -> usize {
        self.metadata.as_ref().map_or(0, Vec::len)
    }

    fn total_size(&self) -> usize {
        self.len + self.framed_metadata_len()
    }

    /// The data bytes segment `index` (1-based) carries. The first segment's share is
    /// reduced by the metadata in front of it (`Resource.py` 310-320).
    fn range(&self, index: usize) -> Range<usize> {
        let len = self.len;
        let first = MAX_SEGMENT_SIZE.saturating_sub(self.framed_metadata_len());
        let (start, end) = match index {
            1 if self.segments == 1 => (0, len),
            1 => (0, first),
            _ => {
                let start = first + (index - 2) * MAX_SEGMENT_SIZE;
                (start, start + MAX_SEGMENT_SIZE)
            }
        };
        start.min(len)..end.min(len)
    }

    fn segment(
        &mut self,
        index: usize,
        random_hash: [u8; RANDOM_HASH_LEN],
        iv: &[u8; IV_LEN],
    ) -> ResourceSender {
        let data = self.data.as_ref().expect("held until the last segment");
        let slice = &data.as_ref()[self.range(index)];
        let bytes = match (&self.metadata, index) {
            (Some(metadata), 1) => Cow::Owned([metadata.as_slice(), slice].concat()),
            _ => Cow::Borrowed(slice),
        };
        let mut out = outgoing(&self.link, &bytes, random_hash, iv, self.compress).with_segment(
            index as i64,
            self.segments as i64,
            self.total_size() as u64,
            self.original_hash,
        );
        out = match self.kind {
            ResourceKind::Data => out,
            ResourceKind::Request(id) => out.with_request(id),
            ResourceKind::Response(id) => out.with_request_id(id),
        };
        // Every segment carries the flag; only the first carries the metadata.
        if self.metadata.is_some() {
            out = out.with_metadata();
        }
        if index == self.segments {
            self.data = None;
        }
        ResourceSender::from_outgoing(self.link.clone(), out).with_timing(self.timing)
    }
}

/// Publishes a resource of any size over a link, as RNS does: up to [`MAX_SEGMENT_SIZE`]
/// bytes go as one [`ResourceSender`]; past it the data splits into segments sharing the
/// first segment's hash as `o`, each advertised once the previous one is proved.
///
/// Only the current segment is sealed and held; `data` is borrowed or owned as the caller
/// chooses, so memory beyond it is bounded by one segment, and it is released once the
/// last segment is built (at once, for a whole resource).
pub struct SegmentedSender<D> {
    plan: Plan<D>,
    current: ResourceSender,
    index: usize,
    served_before: usize,
}

impl<D: AsRef<[u8]>> SegmentedSender<D> {
    /// Prepare to publish `data` (uncompressed) over `link`, with optional `metadata` (one
    /// already-packed msgpack value). `random_hash` and `iv` serve the first segment; later
    /// ones draw both from the `iv` source given to [`on_packet`](Self::on_packet).
    ///
    /// Returns [`Error::CapacityExceeded`] for metadata past
    /// [`METADATA_MAX_SIZE`](crate::resource::METADATA_MAX_SIZE), or too large to leave
    /// room for data in a first segment.
    pub fn new(
        link: Link,
        data: D,
        metadata: Option<&[u8]>,
        kind: ResourceKind,
        random_hash: [u8; RANDOM_HASH_LEN],
        iv: &[u8; IV_LEN],
    ) -> Result<Self> {
        let metadata = metadata.map(pack_metadata).transpose()?;
        let len = data.as_ref().len();
        let framed = metadata.as_ref().map_or(0, Vec::len);
        let segments = segment_count(len + framed);
        if segments > 1 && framed >= MAX_SEGMENT_SIZE {
            return Err(Error::CapacityExceeded);
        }
        let mut plan = Plan {
            link,
            data: Some(data),
            len,
            metadata,
            kind,
            // RNS compresses only data up to its auto-compress limit (`Resource.py` 392).
            compress: len <= DEFAULT_MAX_DECOMPRESSED_SIZE,
            segments,
            original_hash: [0; 32],
            timing: Timing::default(),
        };
        let current = plan.segment(1, random_hash, iv);
        // The first segment's hash, after any collision re-draw, names the whole resource.
        plan.original_hash = current.resource_hash();
        Ok(Self {
            plan,
            current,
            index: 1,
            served_before: 0,
        })
    }

    /// Time every segment's retransmissions from `timing`; see [`Timing`].
    pub fn with_timing(mut self, timing: Timing) -> Self {
        self.plan.timing = timing;
        self.current = self.current.with_timing(timing);
        self
    }

    /// The current segment's advertisement to send at tick `now`, which starts its
    /// advertisement timer; see [`ResourceSender::advertise`].
    pub fn advertise(&mut self, now: u64, iv: &[u8; IV_LEN]) -> Packet {
        self.current.advertise(now, iv)
    }

    /// The current segment's advertisement, sealed, without touching its timer.
    pub fn advertisement(&self, iv: &[u8; IV_LEN]) -> Packet {
        self.current.advertisement(iv)
    }

    /// Handle one inbound packet at tick `now`, as [`ResourceSender::on_packet`]. When it
    /// proves a segment that is not the last, the next segment is prepared and its
    /// advertisement is among the packets returned (`Resource.py` 809-835).
    pub fn on_packet(
        &mut self,
        packet: &Packet,
        now: u64,
        mut iv: impl FnMut() -> [u8; IV_LEN],
    ) -> Vec<Packet> {
        let mut out = self.current.on_packet(packet, now, &mut iv);
        if self.current.is_done() && self.index < self.plan.segments {
            self.index += 1;
            self.served_before += self.current.served_parts();
            let mut random_hash = [0; RANDOM_HASH_LEN];
            random_hash.copy_from_slice(&iv()[..RANDOM_HASH_LEN]);
            self.current = self.plan.segment(self.index, random_hash, &iv());
            out.push(self.current.advertise(now, &iv()));
        }
        out
    }

    /// Run the current segment's watchdog at tick `now`; see [`ResourceSender::poll`].
    pub fn poll(&mut self, now: u64, iv: impl FnMut() -> [u8; IV_LEN]) -> Option<Packet> {
        self.current.poll(now, iv)
    }

    /// The tick at which [`poll`](Self::poll) next has work, while the transfer runs.
    pub fn deadline(&self) -> Option<u64> {
        self.current.deadline()
    }

    /// Whether this sender gave up on a silent receiver; see
    /// [`ResourceSender::timed_out`].
    pub fn timed_out(&self) -> bool {
        self.current.timed_out()
    }

    /// Whether every segment has been proved.
    pub fn is_done(&self) -> bool {
        self.index == self.plan.segments && self.current.is_done()
    }

    /// Whether the transfer was canceled, by either side; see
    /// [`ResourceSender::is_canceled`].
    pub fn is_canceled(&self) -> bool {
        self.current.is_canceled()
    }

    /// Whether the current segment awaits only its proof; see
    /// [`ResourceSender::awaiting_proof`].
    pub fn awaiting_proof(&self) -> bool {
        self.current.awaiting_proof()
    }

    /// A cache request for the current segment's proof; see
    /// [`ResourceSender::cache_request`].
    pub fn cache_request(&mut self) -> Option<Packet> {
        self.current.cache_request()
    }

    /// Cancel the transfer at its current segment; see [`ResourceSender::cancel`].
    pub fn cancel(&mut self, iv: &[u8; IV_LEN]) -> Option<Packet> {
        self.current.cancel(iv)
    }

    /// Whether the receiver has begun requesting the current segment's parts.
    pub fn has_started(&self) -> bool {
        self.current.has_started()
    }

    /// Requested parts served so far, across every segment.
    pub fn served_parts(&self) -> usize {
        self.served_before + self.current.served_parts()
    }

    /// The current segment's 1-based index and the segment count.
    pub fn segment(&self) -> (usize, usize) {
        (self.index, self.plan.segments)
    }

    /// The hash naming the whole resource: the first segment's resource hash.
    pub fn original_hash(&self) -> [u8; 32] {
        self.plan.original_hash
    }

    /// The current segment's resource hash.
    pub fn resource_hash(&self) -> [u8; 32] {
        self.current.resource_hash()
    }
}
