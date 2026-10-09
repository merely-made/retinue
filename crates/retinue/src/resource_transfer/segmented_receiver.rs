//! Receiving a resource of any size as RNS segments (`Resource.py` 702-762).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use super::cancel::cancel_packet;
use super::{ResourceKind, ResourceReceiver, WindowCarry, segment_count};
use crate::Error;
use crate::link::{CTX_RESOURCE_ADV, CTX_RESOURCE_RCL, Link};
use crate::packet::Packet;
use crate::resource::{
    Advertisement, DEFAULT_MAX_DECOMPRESSED_SIZE, FLAG_REQUEST, FLAG_RESPONSE, MAX_SEGMENT_SIZE,
};
use crate::token::IV_LEN;

/// The largest resource a [`SegmentedReceiver`] accepts unless told otherwise: 64 MiB,
/// RNS's auto-compress limit. RNS streams segments to disk; this receiver holds them.
pub const DEFAULT_MAX_RESOURCE_SIZE: usize = DEFAULT_MAX_DECOMPRESSED_SIZE;

/// Bytes of length in front of packed metadata; see [`pack_metadata`](crate::resource::pack_metadata).
const METADATA_FRAMING: usize = 3;

type MakeReceiver = Box<dyn Fn() -> ResourceReceiver + Send + Sync>;
type OfferFilter = Box<dyn Fn(&Advertisement) -> bool + Send + Sync>;

/// The resource being received, fixed by its first advertisement.
struct Offer {
    original_hash: Vec<u8>,
    segments: usize,
    total_size: u64,
    kind: ResourceKind,
    /// The segment in progress, or expected next, 1-based.
    index: usize,
}

/// Receives one resource of any size over a link: a single-segment one as
/// [`ResourceReceiver`] does, and a split one segment by segment, each with its own
/// receiver, appending each segment's data and completing only at the last.
///
/// Each segment must name the first segment's hash as `o`, the expected index, and the
/// first advertisement's segment count and total size; the total is capped (see
/// [`with_max_size`](Self::with_max_size)). Memory is the data so far plus one segment's
/// parts. Each segment's offer passes the per-segment receiver's own limits and accept
/// hook, as RNS asks its `ACCEPT_APP` callback for every advertisement.
///
/// Each segment's receiver starts its request window where the previous segment's ended,
/// as RNS carries the window from one resource to the next through the link.
///
/// Between segments a sender's cancel goes unseen: it names the next segment, whose
/// advertisement has not arrived, and no watchdog runs. The caller's silence limit ends
/// such a transfer.
pub struct SegmentedReceiver {
    link: Link,
    make: MakeReceiver,
    filter: Option<OfferFilter>,
    max_size: usize,
    offer: Option<Offer>,
    /// The receiver of the segment in progress; `None` between segments.
    current: Option<ResourceReceiver>,
    /// The resource hash of the current, or last proved, segment.
    segment_hash: Option<Vec<u8>>,
    data: Vec<u8>,
    metadata: Option<Vec<u8>>,
    /// Bytes received so far, metadata framing included: what `d` counts.
    received: u64,
    proved: usize,
    last_proof: Option<Packet>,
    complete: bool,
    /// Whether the payload has been taken.
    delivered: bool,
    failure: Option<Error>,
    canceled: bool,
    /// Where the last finished segment left the request window.
    carry: Option<WindowCarry>,
}

impl SegmentedReceiver {
    /// A receiver awaiting an advertisement on `link`, building each segment's receiver
    /// with `make` (its window, limits and accept hook).
    pub fn new(link: Link, make: impl Fn() -> ResourceReceiver + Send + Sync + 'static) -> Self {
        Self {
            link,
            make: Box::new(make),
            filter: None,
            max_size: DEFAULT_MAX_RESOURCE_SIZE,
            offer: None,
            current: None,
            segment_hash: None,
            data: Vec::new(),
            metadata: None,
            received: 0,
            proved: 0,
            last_proof: None,
            complete: false,
            delivered: false,
            failure: None,
            canceled: false,
            carry: None,
        }
    }

    /// Refuse a resource whose advertised total size exceeds `max_size` bytes. The sender
    /// gets a receiver cancel and this receiver fails with [`Error::CapacityExceeded`].
    /// A body larger than advertised fails too, and a compressed one stops inflating there.
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    /// Silently ignore a first offer `filter` refuses, as RNS ignores a request
    /// advertisement on a link whose destination has no request handlers.
    pub fn with_filter(
        mut self,
        filter: impl Fn(&Advertisement) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.filter = Some(Box::new(filter));
        self
    }

    /// Handle one inbound packet from the sender at tick `now`, returning packets to send.
    pub fn on_packet(
        &mut self,
        packet: &Packet,
        now: u64,
        mut iv: impl FnMut() -> [u8; IV_LEN],
    ) -> Vec<Packet> {
        if packet.context == CTX_RESOURCE_ADV {
            return self.on_offer(packet, now, &mut iv);
        }
        if self.ended() {
            return vec![];
        }
        let Some(current) = self.current.as_mut() else {
            return vec![];
        };
        let replies = current.on_packet(packet, now, &mut iv);
        self.settle();
        replies
    }

    fn ended(&self) -> bool {
        self.complete || self.failure.is_some() || self.canceled
    }

    /// An advertisement: the first offer, the next segment, or a re-sent one.
    fn on_offer(
        &mut self,
        packet: &Packet,
        now: u64,
        iv: &mut impl FnMut() -> [u8; IV_LEN],
    ) -> Vec<Packet> {
        let Some(adv) = self
            .link
            .decrypt(packet)
            .ok()
            .and_then(|plain| Advertisement::parse(&plain).ok())
            .filter(|adv| adv.resource_hash.len() == 32)
        else {
            return vec![];
        };
        if self.segment_hash.as_ref() == Some(&adv.resource_hash) {
            if !self.ended()
                && let Some(current) = self.current.as_mut()
            {
                return current.on_packet(packet, now, &mut *iv);
            }
            // The segment just proved, offered again: its proof was lost.
            if self.failure.is_none() && !self.canceled {
                return self.last_proof.clone().into_iter().collect();
            }
        }
        if self.complete {
            return vec![];
        }
        // Refused: after an end, and for another resource while a segment is in progress,
        // which leaves that segment be.
        if self.ended() || self.current.is_some() {
            return vec![self.refusal(&adv, iv)];
        }
        let Some(kind) = kind_of(&adv) else {
            return vec![];
        };
        let index = match &self.offer {
            None => {
                if self.filter.as_ref().is_some_and(|filter| !filter(&adv)) {
                    return vec![];
                }
                if adv.data_size > self.max_size as u64 {
                    return self.fail(Error::CapacityExceeded, &adv, iv);
                }
                if !is_first_segment(&adv) {
                    return self.fail(Error::ResourceCorrupt, &adv, iv);
                }
                1
            }
            // Another resource offered mid-way through this one: refuse it alone.
            Some(offer) if adv.original_hash != offer.original_hash => {
                return vec![self.refusal(&adv, iv)];
            }
            Some(offer) => {
                let expected = adv.i == offer.index as i64
                    && adv.l == offer.segments as i64
                    && adv.data_size == offer.total_size
                    && kind == offer.kind;
                if !expected {
                    return self.fail(Error::ResourceCorrupt, &adv, iv);
                }
                offer.index
            }
        };
        let mut receiver = (self.make)();
        if let Some(carry) = self.carry {
            receiver = receiver.with_carry(carry);
        }
        receiver = if adv.l > 1 {
            // A segment inflates to at most the segment size.
            receiver
                .allow_segments()
                .with_max_decompressed_size(MAX_SEGMENT_SIZE)
        } else {
            // A whole resource inflates to at most its advertised size, already within
            // `max_size`: an understated `d` cannot buy more memory.
            receiver.cap_decompressed(adv.data_size as usize)
        };
        let replies = receiver.on_packet(packet, now, &mut *iv);
        if let Some(error) = receiver.failure() {
            self.failure = Some(error);
        } else if !replies.is_empty() {
            self.offer.get_or_insert(Offer {
                original_hash: adv.original_hash.clone(),
                segments: adv.l as usize,
                total_size: adv.data_size,
                kind,
                index,
            });
            self.segment_hash = Some(adv.resource_hash);
            self.current = Some(receiver);
        }
        replies
    }

    /// Fold the current segment's state in after a packet: a failure or cancel ends the
    /// resource; a completed segment is appended, and the last one completes it.
    fn settle(&mut self) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if let Some(error) = current.failure() {
            self.failure = Some(error);
            return;
        }
        if current.is_canceled() {
            self.canceled = true;
            return;
        }
        let Some((data, metadata)) = current.take_payload() else {
            return;
        };
        self.proved += 1;
        self.last_proof = current.proof_packet();
        self.carry = Some(current.carry());
        let framing = metadata.as_ref().map_or(0, |m| METADATA_FRAMING + m.len());
        self.received += (data.len() + framing) as u64;
        if metadata.is_some() {
            self.metadata = metadata;
        }
        if self.data.is_empty() {
            self.data = data;
        } else {
            self.data.extend_from_slice(&data);
        }
        let offer = self.offer.as_mut().expect("a segment implies an offer");
        let split = offer.segments > 1;
        if self.received > offer.total_size {
            self.failure = Some(Error::ResourceCorrupt);
        } else if offer.index < offer.segments {
            offer.index += 1;
            self.current = None;
        } else if split && self.received != offer.total_size {
            self.failure = Some(Error::ResourceCorrupt);
        } else {
            self.complete = true;
        }
        if self.failure.is_some() {
            self.data = Vec::new();
        }
    }

    /// Fail on an offer, refusing it.
    fn fail(
        &mut self,
        error: Error,
        adv: &Advertisement,
        iv: &mut impl FnMut() -> [u8; IV_LEN],
    ) -> Vec<Packet> {
        self.failure = Some(error);
        self.current = None;
        self.data = Vec::new();
        vec![self.refusal(adv, iv)]
    }

    fn refusal(&self, adv: &Advertisement, iv: &mut impl FnMut() -> [u8; IV_LEN]) -> Packet {
        cancel_packet(&self.link, CTX_RESOURCE_RCL, &adv.resource_hash, &iv())
    }

    /// Run the current segment's watchdog at tick `now`; see [`ResourceReceiver::poll`].
    /// Between segments there is nothing to do: the sender advertises.
    pub fn poll(&mut self, now: u64, iv: impl FnMut() -> [u8; IV_LEN]) -> Vec<Packet> {
        if self.ended() {
            return vec![];
        }
        let Some(current) = self.current.as_mut() else {
            return vec![];
        };
        let replies = current.poll(now, iv);
        self.settle();
        replies
    }

    /// The tick at which [`poll`](Self::poll) next has work: the current segment's
    /// deadline. `None` between segments and once the transfer has ended.
    pub fn deadline(&self) -> Option<u64> {
        if self.ended() {
            return None;
        }
        self.current.as_ref()?.deadline()
    }

    /// What this transfer leaves its link for the next, once a segment has run; see
    /// [`ResourceReceiver::carry`].
    pub fn carry(&self) -> Option<WindowCarry> {
        self.current
            .as_ref()
            .map(ResourceReceiver::carry)
            .or(self.carry)
    }

    /// Cancel the transfer: the sealed receiver cancel for the segment in progress, if
    /// any. Between segments the next advertisement is refused instead.
    pub fn cancel(&mut self, iv: &[u8; IV_LEN]) -> Option<Packet> {
        if self.complete || self.failure.is_some() || self.canceled {
            return None;
        }
        self.canceled = true;
        self.data = Vec::new();
        self.current.as_mut()?.cancel(iv)
    }

    /// Why the transfer failed, if it has; see [`ResourceReceiver::failure`]. A segment
    /// out of sequence, or sizes that disagree with the first advertisement, fail with
    /// [`Error::ResourceCorrupt`].
    pub fn failure(&self) -> Option<Error> {
        self.failure
    }

    /// Whether the transfer was canceled, by the sender or by [`cancel`](Self::cancel).
    pub fn is_canceled(&self) -> bool {
        self.canceled
    }

    /// Whether every segment has been received, verified and proved.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// How many segments have been proved so far.
    pub fn segments_proved(&self) -> usize {
        self.proved
    }

    /// The most recent segment proof sent, for the sender's cache request.
    pub fn last_proof(&self) -> Option<&Packet> {
        self.last_proof.as_ref()
    }

    /// The proof of the final segment, once complete.
    pub fn proof_packet(&self) -> Option<Packet> {
        self.last_proof.clone().filter(|_| self.complete)
    }

    /// What the resource carries, once its first advertisement is accepted.
    pub fn kind(&self) -> Option<ResourceKind> {
        self.offer.as_ref().map(|offer| offer.kind)
    }

    /// The `(index, count)` of the segment in progress or expected next.
    pub fn segment(&self) -> Option<(usize, usize)> {
        self.offer
            .as_ref()
            .map(|offer| (offer.index, offer.segments))
    }

    /// The whole payload, once complete. Metadata is not part of it.
    pub fn data(&self) -> Option<&[u8]> {
        (self.complete && !self.delivered).then_some(self.data.as_slice())
    }

    /// The packed metadata the sender attached, once complete.
    pub fn metadata(&self) -> Option<&[u8]> {
        self.metadata.as_deref().filter(|_| self.complete)
    }

    /// Take the payload and its metadata out of a completed receiver. The proof stays.
    pub fn take_payload(&mut self) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        if !self.complete || self.delivered {
            return None;
        }
        self.delivered = true;
        Some((core::mem::take(&mut self.data), self.metadata.take()))
    }
}

/// What an advertisement says it carries, or `None` if its request id is malformed.
fn kind_of(adv: &Advertisement) -> Option<ResourceKind> {
    let id = || <[u8; 16]>::try_from(adv.q.as_deref()?).ok();
    Some(if adv.flags & FLAG_REQUEST != 0 {
        ResourceKind::Request(id()?)
    } else if adv.flags & FLAG_RESPONSE != 0 {
        ResourceKind::Response(id()?)
    } else {
        ResourceKind::Data
    })
}

/// Whether `adv` can begin a resource: a whole one, or the first of a split one whose
/// segment count matches its total size and whose original hash is its own.
fn is_first_segment(adv: &Advertisement) -> bool {
    adv.i == 1
        && (adv.l == 1
            || (adv.l > 1
                && usize::try_from(adv.data_size)
                    .is_ok_and(|size| segment_count(size) as i64 == adv.l)
                && adv.original_hash == adv.resource_hash))
}
