//! The receiving half of a resource transfer.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use super::AcceptHook;
use super::cancel::{cancel_packet, names_resource};
use crate::Error;
use crate::link::{
    CTX_RESOURCE, CTX_RESOURCE_ADV, CTX_RESOURCE_HMU, CTX_RESOURCE_ICL, CTX_RESOURCE_RCL,
    CTX_RESOURCE_REQ, Link,
};
use crate::packet::Packet;
#[cfg(not(feature = "compression"))]
use crate::resource::FLAG_COMPRESSED;
use crate::resource::{
    Advertisement, DEFAULT_MAX_DECOMPRESSED_SIZE, FLAG_RESPONSE, Incoming, parse_hmu,
    split_metadata,
};
use crate::token::IV_LEN;

/// Receives one resource over a link: on the advertisement it requests parts, collects them,
/// solicits more hashmap when needed, then reassembles, opens, verifies, and proves.
pub struct ResourceReceiver {
    link: Link,
    inc: Option<Incoming>,
    data: Option<Vec<u8>>,
    metadata: Option<Vec<u8>>,
    /// `(resource hash, proof)` once the payload is verified, for proof retransmission.
    proved: Option<([u8; 32], [u8; 32])>,
    canceled: bool,
    request_window: usize,
    /// The most parts this receiver will accept for one segment, so a peer's advertised
    /// count cannot decide our reassembly memory. A board sets it far lower.
    max_parts: usize,
    /// The most bytes a compressed resource may inflate to before the transfer fails.
    max_decompressed: usize,
    /// The largest advertised data size accepted, if bounded.
    max_data_size: Option<usize>,
    accept: Option<AcceptHook>,
    /// Why this receiver gave up, once it has. A failed receiver answers nothing more.
    failure: Option<Error>,
    outstanding: usize,
    response_request_id: Option<[u8; 16]>,
    /// Whether one segment of a split resource is accepted; see
    /// [`SegmentedReceiver`](super::SegmentedReceiver).
    segmented: bool,
}

impl ResourceReceiver {
    /// A receiver awaiting an advertisement on `link`.
    pub fn new(link: Link) -> Self {
        Self::with_request_window(link, crate::resource::HASHMAP_MAX_PARTS)
    }

    /// A receiver that asks for at most `request_window` parts per turn.
    pub fn with_request_window(link: Link, request_window: usize) -> Self {
        Self::with_limits(link, request_window, crate::resource::DEFAULT_MAX_PARTS)
    }

    /// A receiver with an explicit ceiling on both the request window and the size of a
    /// resource it will accept at all.
    ///
    /// `max_parts` is the one a board must set: it is the point where a peer's advertised
    /// size stops being this node's problem. See the plan's N1 notes on the resource cap.
    pub fn with_limits(link: Link, request_window: usize, max_parts: usize) -> Self {
        Self {
            link,
            inc: None,
            data: None,
            metadata: None,
            proved: None,
            canceled: false,
            request_window: request_window.clamp(1, crate::resource::HASHMAP_MAX_PARTS),
            max_parts: max_parts.max(1),
            max_decompressed: DEFAULT_MAX_DECOMPRESSED_SIZE,
            max_data_size: None,
            accept: None,
            failure: None,
            outstanding: 0,
            response_request_id: None,
            segmented: false,
        }
    }

    /// Set the most bytes a compressed resource may decompress to. The default is
    /// [`DEFAULT_MAX_DECOMPRESSED_SIZE`] (64 MiB, as RNS). Past it the transfer fails with
    /// [`Error::DecompressionLimit`] and the sender is told to stop.
    pub fn with_max_decompressed_size(mut self, max_decompressed: usize) -> Self {
        self.max_decompressed = max_decompressed;
        self
    }

    /// Refuse an offer whose advertised data size exceeds `max_data_size` bytes, as RNS
    /// refuses a response past a request's `max_response_size`: the sender gets a receiver
    /// cancel and this receiver fails with [`Error::CapacityExceeded`]. Decompression is
    /// bounded by the same size, so an offer that understates its size fails too.
    pub fn with_max_data_size(mut self, max_data_size: usize) -> Self {
        self.max_data_size = Some(max_data_size);
        self.max_decompressed = self.max_decompressed.min(max_data_size);
        self
    }

    /// Decide on each new offer with `accept`, as RNS's `ACCEPT_APP` strategy does. An
    /// offer it refuses gets a receiver cancel, and this receiver fails with
    /// [`Error::ResourceRejected`].
    pub fn with_accept(
        mut self,
        accept: impl Fn(&Advertisement) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.accept = Some(Box::new(accept));
        self
    }

    /// Accept one segment of a split resource as a whole transfer: the caller accumulates
    /// the segments.
    pub(super) fn allow_segments(mut self) -> Self {
        self.segmented = true;
        self
    }

    /// The part ceiling this receiver enforces.
    pub fn max_parts(&self) -> usize {
        self.max_parts
    }

    /// Why this receiver failed, if it has: [`Error::MultiSegmentResource`] for an offer
    /// it cannot reassemble whole (see [`SegmentedReceiver`](super::SegmentedReceiver)), [`Error::CapacityExceeded`] for one past its part or
    /// size ceiling, [`Error::ResourceRejected`] for one its accept hook refused,
    /// [`Error::DecompressionLimit`] for a body that inflated past its limit, and
    /// [`Error::ResourceCorrupt`] for one that failed to open or verify. The sender has
    /// been sent a cancel; a failed receiver never yields data.
    pub fn failure(&self) -> Option<Error> {
        self.failure
    }

    /// Handle one inbound packet from the sender, returning packets to send. On the
    /// advertisement it begins the transfer and requests parts; on a part it accepts it and
    /// requests more (or proves, once complete); on an HMU it ingests the new hashes and
    /// requests the newly-known parts.
    pub fn on_packet(
        &mut self,
        packet: &Packet,
        mut iv: impl FnMut() -> [u8; IV_LEN],
    ) -> Vec<Packet> {
        // A failed or canceled receiver only answers a (re-sent) advertisement, with
        // another refusal.
        if (self.failure.is_some() || self.canceled) && packet.context != CTX_RESOURCE_ADV {
            return vec![];
        }
        match packet.context {
            CTX_RESOURCE_ADV => {
                let Ok(plain) = self.link.decrypt(packet) else {
                    return vec![];
                };
                let Ok(adv) = Advertisement::parse(&plain) else {
                    return vec![];
                };
                if adv.resource_hash.len() != 32 {
                    return vec![];
                }
                if self.failure.is_some() || self.canceled {
                    return vec![self.cancel_packet(&adv.resource_hash, &mut iv)];
                }
                let is_new = self
                    .inc
                    .as_ref()
                    .is_none_or(|current| current.resource_hash()[..] != adv.resource_hash[..]);
                if !is_new {
                    // A re-sent offer of the resource already proved means the sender lost
                    // the proof: send it again rather than nothing.
                    return match self.proof_packet() {
                        Some(proof) => vec![proof],
                        None => self.next_requests(&mut iv),
                    };
                }
                let response_request_id = if adv.flags & FLAG_RESPONSE != 0 {
                    let Some(request_id) = adv.q.as_deref() else {
                        return vec![];
                    };
                    let Ok(request_id) = <[u8; 16]>::try_from(request_id) else {
                        return vec![];
                    };
                    Some(request_id)
                } else {
                    None
                };
                let incoming = match self.admit(&adv) {
                    Ok(incoming) => incoming,
                    // A malformed advertisement is dropped, as RNS drops one it cannot
                    // decode; there is nothing trustworthy to name in a refusal.
                    Err(Error::BadRequest) => return vec![],
                    Err(refusal) => {
                        // An offer refused while another transfer runs leaves that one be.
                        if self.inc.is_none() {
                            self.failure = Some(refusal);
                        }
                        return vec![self.cancel_packet(&adv.resource_hash, &mut iv)];
                    }
                };
                self.inc = Some(incoming);
                self.outstanding = 0;
                self.response_request_id = response_request_id;
                self.next_requests(&mut iv)
            }
            CTX_RESOURCE => {
                if self.is_complete() {
                    return vec![];
                }
                let Some(inc) = self.inc.as_mut() else {
                    return vec![];
                };
                if inc.accept_part(&packet.payload) {
                    self.outstanding = self.outstanding.saturating_sub(1);
                }
                if inc.is_complete() {
                    self.finish(&mut iv)
                } else if self.outstanding == 0 {
                    self.next_requests(&mut iv)
                } else {
                    vec![]
                }
            }
            CTX_RESOURCE_HMU => {
                let Ok(plain) = self.link.decrypt(packet) else {
                    return vec![];
                };
                let Ok(hmu) = parse_hmu(&plain) else {
                    return vec![];
                };
                if let Some(inc) = self
                    .inc
                    .as_mut()
                    .filter(|inc| inc.resource_hash() == hmu.resource_hash)
                {
                    inc.ingest_hmu(&hmu);
                }
                self.next_requests(&mut iv)
            }
            CTX_RESOURCE_ICL => {
                if !self.is_complete()
                    && let Some(inc) = self.inc.as_ref()
                    && names_resource(&self.link, packet, &inc.resource_hash())
                {
                    self.canceled = true;
                    self.inc = None;
                }
                vec![]
            }
            _ => vec![],
        }
    }

    /// Check a new offer against this receiver's limits and accept hook, and begin
    /// receiving it. Every error but [`Error::BadRequest`] is a refusal to answer with a
    /// receiver cancel.
    fn admit(&self, adv: &Advertisement) -> Result<Incoming, Error> {
        // A resource past RNS's segment size arrives as `l` advertisements, one per
        // segment. Alone, this receiver would hand back the first segment's data as if it
        // were the whole resource.
        if adv.l > 1 && !self.segmented {
            return Err(Error::MultiSegmentResource);
        }
        #[cfg(not(feature = "compression"))]
        if adv.flags & FLAG_COMPRESSED != 0 {
            return Err(Error::Unsupported);
        }
        if self
            .max_data_size
            .is_some_and(|max| adv.data_size > max as u64)
        {
            return Err(Error::CapacityExceeded);
        }
        // Past the part ceiling is refused outright: the peer chose that number.
        let incoming = Incoming::new_with_max_parts(adv, self.max_parts)?;
        if self.accept.as_ref().is_some_and(|accept| !accept(adv)) {
            return Err(Error::ResourceRejected);
        }
        Ok(incoming.with_window(self.request_window))
    }

    /// Re-emit the outstanding request (for loss recovery when a request or its parts were
    /// dropped). Empty once complete or before the advertisement.
    pub fn retransmit(&mut self, iv: impl FnMut() -> [u8; IV_LEN]) -> Vec<Packet> {
        if self.canceled || self.failure.is_some() {
            return vec![];
        } else if self.is_complete() {
            // Already complete: re-prove in case the proof was lost.
            return self.proof_packet().into_iter().collect();
        }
        let mut iv = iv;
        self.next_requests(&mut iv)
    }

    /// Cancel a transfer in progress: the sealed receiver cancel (`RESOURCE_RCL`) to send,
    /// or `None` if no transfer is running. Nothing further is requested.
    pub fn cancel(&mut self, iv: &[u8; IV_LEN]) -> Option<Packet> {
        if self.is_complete() || self.failure.is_some() || self.canceled {
            return None;
        }
        let inc = self.inc.take()?;
        self.canceled = true;
        Some(cancel_packet(
            &self.link,
            CTX_RESOURCE_RCL,
            &inc.resource_hash(),
            iv,
        ))
    }

    /// Whether the transfer was canceled: by the sender's cancel naming it, or by
    /// [`cancel`](Self::cancel).
    pub fn is_canceled(&self) -> bool {
        self.canceled
    }

    /// The requests to send now: the known-but-missing parts, or a hashmap solicitation when
    /// the known hashes are collected but more parts remain.
    fn next_requests(&mut self, iv: &mut impl FnMut() -> [u8; IV_LEN]) -> Vec<Packet> {
        let Some(inc) = self.inc.as_ref().filter(|_| self.proved.is_none()) else {
            return vec![];
        };
        let missing = inc.missing_known();
        if !missing.is_empty() {
            let wanted = &missing[..missing.len().min(self.request_window)];
            self.outstanding = wanted.len();
            vec![
                self.link
                    .sealed_packet(CTX_RESOURCE_REQ, &inc.request(wanted), &iv()),
            ]
        } else if inc.needs_hmu() {
            self.outstanding = 0;
            vec![
                self.link
                    .sealed_packet(CTX_RESOURCE_REQ, &inc.solicit_hmu(), &iv()),
            ]
        } else {
            vec![]
        }
    }

    /// Reassemble, open, verify, and build the proof packet. Records the payload, and its
    /// metadata if the advertisement flagged some. A body that fails here fails the
    /// transfer, and the sender is told to stop.
    fn finish(&mut self, iv: &mut impl FnMut() -> [u8; IV_LEN]) -> Vec<Packet> {
        let inc = self
            .inc
            .as_mut()
            .expect("complete implies an advertisement");
        let hash = inc.resource_hash();
        let recovered = inc
            .take_token()
            .and_then(|token| self.link.open(&token))
            .and_then(|decrypted| inc.recover_with_limit(&decrypted, self.max_decompressed))
            .and_then(|mut data| {
                // The proof covers the data as hashed, metadata and all.
                let proof = inc.proof(&data);
                let metadata = inc
                    .has_metadata()
                    .then(|| split_metadata(&mut data))
                    .transpose()?;
                Ok((data, metadata, proof))
            });
        match recovered {
            Ok((data, metadata, proof)) => {
                self.data = Some(data);
                self.metadata = metadata;
                self.proved = Some((hash, proof));
                vec![self.link.resource_proof_packet(&hash, &proof)]
            }
            Err(error) => {
                // A bz2 bomb, an oversized body, or one that does not open or verify: fail,
                // release the parts, and tell the sender to stop.
                self.failure = Some(match error {
                    Error::DecompressionLimit | Error::Unsupported => error,
                    _ => Error::ResourceCorrupt,
                });
                self.inc = None;
                vec![self.cancel_packet(&hash, iv)]
            }
        }
    }

    /// The proof packet for the verified payload, once there is one: what a sender's cache
    /// request asks to have re-sent.
    pub fn proof_packet(&self) -> Option<Packet> {
        let (hash, proof) = self.proved.as_ref()?;
        Some(self.link.resource_proof_packet(hash, proof))
    }

    /// A receiver cancel (`RESOURCE_RCL`) for `resource_hash`, sealed as RNS sends it.
    fn cancel_packet(&self, resource_hash: &[u8], iv: &mut impl FnMut() -> [u8; IV_LEN]) -> Packet {
        cancel_packet(&self.link, CTX_RESOURCE_RCL, resource_hash, &iv())
    }

    /// The recovered payload, once the transfer is complete and verified. Metadata the
    /// sender attached is not part of it; see [`metadata`](Self::metadata).
    pub fn data(&self) -> Option<&[u8]> {
        self.data.as_deref()
    }

    /// The packed (msgpack) metadata the sender attached, once the transfer is complete.
    pub fn metadata(&self) -> Option<&[u8]> {
        self.metadata.as_deref()
    }

    /// Take the recovered payload and its metadata out of a completed receiver, without
    /// copying them. The proof stays available.
    pub fn take_payload(&mut self) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let data = self.data.take()?;
        Some((data, self.metadata.take()))
    }

    /// Request id carried by a response Resource advertisement.
    pub fn response_request_id(&self) -> Option<[u8; 16]> {
        self.response_request_id
    }

    /// Whether the payload has been fully received and verified.
    pub fn is_complete(&self) -> bool {
        self.proved.is_some()
    }
}
