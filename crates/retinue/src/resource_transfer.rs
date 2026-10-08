//! Driving a resource transfer over a [`Link`]: the sans-io sender/receiver pair that runs
//! the resource codec ([`crate::resource`]) over link packets.
//!
//! A *resource* is RNS's segmented transfer of a payload too large for one packet. The codec
//! and its two state machines ([`Outgoing`], [`Incoming`]) have historical RNS 1.3.8 wire
//! fixtures in `resource.rs`; current local transfer gates use the live-oracle pin in
//! `oracle/requirements.txt`. This module moves their packets across a link, the way
//! [`crate::reliable`] drives the `Channel`/`Buffer` codec.
//!
//! # Wire, by link context byte
//!
//! ```text
//! 0x02 RESOURCE_ADV   the advertisement (msgpack), sealed; (re)sent until the receiver responds
//! 0x03 RESOURCE_REQ   the receiver's request for parts / solicitation for more hashmap, sealed
//! 0x01 RESOURCE       one part: a raw slice of the sealed token, framed (not re-sealed)
//! 0x04 RESOURCE_HMU   a hashmap update for a resource with more parts than one advert carries
//! 0x05 RESOURCE_PRF   the receiver's proof of receipt, a PROOF-type packet, unencrypted:
//!                     resource_hash(32) || proof(32)
//! 0x06 RESOURCE_ICL   the initiator cancels, sealed: resource_hash(32)
//! 0x07 RESOURCE_RCL   the receiver cancels or rejects, sealed: resource_hash(32)
//! 0x08 CACHE_REQUEST  a sender awaiting its proof asks for it again, unencrypted:
//!                     the proof packet's full hash(32)
//! ```
//!
//! The payload is sealed into the token **once** (`link.seal(content)`), then split into
//! parts, so a part is a byte-slice of the already-encrypted token and rides framed; the
//! receiver reassembles the parts verbatim into the token and opens it once. Control packets
//! (advertisement, request, hashmap update, cancels) are sealed; the proof, carrying only
//! public hashes, rides unencrypted in a PROOF-type packet, the only form RNS accepts. A
//! sender still accepts the DATA-type proof older retinue receivers sent.
//!
//! Either side ends a transfer it will not finish with a sealed cancel naming the resource,
//! as RNS does (`Resource.cancel`, `Resource.reject`): the receiver on a refused offer, a
//! corrupt or oversized body, or a local cancel; the sender on a local cancel. A cancel is
//! honoured only if it decrypts on the link and names the resource in progress.
//!
//! Both halves are sans-io: [`ResourceSender::on_packet`] / [`ResourceReceiver::on_packet`]
//! take a received packet and return packets to send, and the retransmit helpers re-emit on a
//! stall. A caller (a link task, or a virtual-clock loss test) pumps them.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::link::{
    CTX_CACHE_REQUEST, CTX_RESOURCE, CTX_RESOURCE_ADV, CTX_RESOURCE_HMU, CTX_RESOURCE_ICL,
    CTX_RESOURCE_PRF, CTX_RESOURCE_RCL, CTX_RESOURCE_REQ, Link,
};
use crate::packet::Packet;
#[cfg(not(feature = "compression"))]
use crate::resource::FLAG_COMPRESSED;
#[cfg(feature = "compression")]
use crate::resource::compress;
use crate::resource::{
    Advertisement, DEFAULT_MAX_DECOMPRESSED_SIZE, FLAG_RESPONSE, Incoming, Outgoing,
    RANDOM_HASH_LEN, SDU, content, pack_metadata, parse_hmu, parse_proof, parse_request,
    split_metadata,
};
use crate::token::IV_LEN;

/// How many times a sender that has sent every part asks for its missing proof with a
/// cache request. RNS resets `retries_left` to 3 on entering `AWAITING_PROOF`.
pub const PROOF_CACHE_REQUESTS: u8 = 3;

/// Decides whether to accept an advertised resource, as an RNS link's `ACCEPT_APP`
/// callback does: it sees the advertisement (sizes, part count, flags) before any part is
/// requested. A refused offer is answered with a receiver cancel.
pub type AcceptHook = Box<dyn Fn(&Advertisement) -> bool + Send + Sync>;

/// A sealed cancel naming `resource_hash`: `RESOURCE_RCL` from a receiver, `RESOURCE_ICL`
/// from the initiator.
fn cancel_packet(link: &Link, context: u8, resource_hash: &[u8], iv: &[u8; IV_LEN]) -> Packet {
    link.sealed_packet(context, resource_hash, iv)
}

/// Whether `packet` decrypts on `link` and names `resource_hash`, as RNS matches a cancel
/// to its resource.
fn names_resource(link: &Link, packet: &Packet, resource_hash: &[u8; 32]) -> bool {
    link.decrypt(packet)
        .is_ok_and(|plain| plain.get(..32) == Some(resource_hash.as_slice()))
}

/// Reject an advertised resource without receiving any of it: RNS's `Resource.reject`.
///
/// Returns the sealed receiver cancel naming the advertised resource, or `None` if
/// `advertisement` does not decrypt on `link` to a well-formed advertisement.
pub fn reject(link: &Link, advertisement: &Packet, iv: &[u8; IV_LEN]) -> Option<Packet> {
    let plain = link.decrypt(advertisement).ok()?;
    let adv = Advertisement::parse(&plain).ok()?;
    (adv.resource_hash.len() == 32)
        .then(|| cancel_packet(link, CTX_RESOURCE_RCL, &adv.resource_hash, iv))
}

/// Publishes one resource over a link: advertises it, serves part requests and hashmap
/// updates, and completes when the receiver's proof of receipt arrives.
pub struct ResourceSender {
    link: Link,
    out: Outgoing,
    hash_window: usize,
    started: bool,
    served_parts: usize,
    /// Which parts have been sent at least once, and how many have not.
    sent: Vec<bool>,
    unsent: usize,
    cache_requests_left: u8,
    done: bool,
    canceled: bool,
}

impl ResourceSender {
    /// Prepare to publish `data` (uncompressed) over `link`. `random_hash` salts the resource
    /// and map hashes; `iv` seals the token (it must not repeat for the link key).
    pub fn publish(
        link: Link,
        data: &[u8],
        random_hash: [u8; RANDOM_HASH_LEN],
        iv: &[u8; IV_LEN],
    ) -> Self {
        Self::prepare(link, data, random_hash, iv, None, false)
    }

    /// Publish `data` with metadata: `metadata` is one already-packed msgpack value, which
    /// rides in front of the data and reaches an RNS receiver as `resource.metadata`.
    /// Returns [`Error::CapacityExceeded`] past
    /// [`METADATA_MAX_SIZE`](crate::resource::METADATA_MAX_SIZE).
    pub fn publish_with_metadata(
        link: Link,
        data: &[u8],
        metadata: &[u8],
        random_hash: [u8; RANDOM_HASH_LEN],
        iv: &[u8; IV_LEN],
    ) -> Result<Self, Error> {
        let mut framed = pack_metadata(metadata)?;
        framed.extend_from_slice(data);
        Ok(Self::prepare(link, &framed, random_hash, iv, None, true))
    }

    /// Prepare a Resource whose advertisement binds it to a request id.
    pub fn respond(
        link: Link,
        data: &[u8],
        request_id: [u8; 16],
        random_hash: [u8; RANDOM_HASH_LEN],
        iv: &[u8; IV_LEN],
    ) -> Self {
        Self::prepare(link, data, random_hash, iv, Some(request_id), false)
    }

    fn prepare(
        link: Link,
        data: &[u8],
        random_hash: [u8; RANDOM_HASH_LEN],
        iv: &[u8; IV_LEN],
        request_id: Option<[u8; 16]>,
        has_metadata: bool,
    ) -> Self {
        #[cfg(feature = "compression")]
        let (transfer, compressed) = {
            let encoded = compress(data);
            if encoded.len() < data.len() {
                (content(&encoded, &random_hash), true)
            } else {
                (content(data, &random_hash), false)
            }
        };
        #[cfg(not(feature = "compression"))]
        let (transfer, compressed) = (content(data, &random_hash), false);
        let token = link.seal(&transfer, iv);
        drop(transfer);
        let part_size = (link.mtu() as usize)
            .saturating_sub(crate::packet::HEADER_MIN_LEN)
            .clamp(1, SDU);
        let mut out = Outgoing::from_token(data, token, random_hash, compressed, part_size);
        if let Some(request_id) = request_id {
            out = out.with_request_id(request_id);
        }
        if has_metadata {
            out = out.with_metadata();
        }
        let mtu = link.mtu() as usize;
        let mut hash_window = out
            .total_parts()
            .clamp(1, crate::resource::HASHMAP_MAX_PARTS);
        while hash_window > 1 {
            let packed = out.advertisement_with_hash_limit(hash_window).pack();
            let probe = link.sealed_packet(CTX_RESOURCE_ADV, &packed, &[0_u8; IV_LEN]);
            if probe.encoded_len() <= mtu {
                break;
            }
            hash_window -= 1;
        }
        let parts = out.total_parts();
        Self {
            link,
            out,
            hash_window,
            started: false,
            served_parts: 0,
            sent: vec![false; parts],
            unsent: parts,
            cache_requests_left: PROOF_CACHE_REQUESTS,
            done: false,
            canceled: false,
        }
    }

    /// The advertisement packet, sealed. (Re)send it until the receiver responds.
    pub fn advertisement(&self, iv: &[u8; IV_LEN]) -> Packet {
        self.link.sealed_packet(
            CTX_RESOURCE_ADV,
            &self
                .out
                .advertisement_with_hash_limit(self.hash_window)
                .pack(),
            iv,
        )
    }

    /// Handle one inbound packet from the receiver, returning packets to send:
    /// a request yields the requested parts (and, if it solicited more hashmap, an HMU); a
    /// valid proof completes the transfer, and a receiver cancel naming this resource ends
    /// it.
    ///
    /// The proof is matched by context alone, so both the PROOF-type packet RNS sends and
    /// the DATA-type one older retinue receivers sent complete the transfer.
    pub fn on_packet(
        &mut self,
        packet: &Packet,
        mut iv: impl FnMut() -> [u8; IV_LEN],
    ) -> Vec<Packet> {
        if self.done || self.canceled {
            return vec![];
        }
        match packet.context {
            CTX_RESOURCE_REQ => {
                let Ok(plain) = self.link.decrypt(packet) else {
                    return vec![];
                };
                let Ok(req) = parse_request(&plain) else {
                    return vec![];
                };
                if req.resource_hash != self.out.resource_hash() {
                    return vec![];
                }
                self.started = true;
                let mut out = Vec::new();
                // Serve every part whose map hash we hold, framed (already encrypted in-token).
                for index in self.out.requested_indices(&req) {
                    let Some(part) = self.out.part(index) else {
                        continue;
                    };
                    out.push(self.link.framed_packet(CTX_RESOURCE, part.to_vec()));
                    self.served_parts += 1;
                    if !core::mem::replace(&mut self.sent[index], true) {
                        self.unsent -= 1;
                    }
                }
                // An exhausted request wants the next slice of the hashmap.
                if req.exhausted
                    && let Some(last) = req.last_map_hash
                {
                    let hmu = self.out.hmu_after_with_hash_limit(&last, self.hash_window);
                    out.push(self.link.sealed_packet(CTX_RESOURCE_HMU, &hmu, &iv()));
                }
                if self.awaiting_proof() {
                    self.cache_requests_left = PROOF_CACHE_REQUESTS;
                }
                out
            }
            CTX_RESOURCE_PRF => {
                if parse_proof(&packet.payload)
                    == Some((self.out.resource_hash(), self.out.expected_proof()))
                {
                    self.done = true;
                }
                vec![]
            }
            CTX_RESOURCE_RCL => {
                if names_resource(&self.link, packet, &self.out.resource_hash()) {
                    self.canceled = true;
                }
                vec![]
            }
            _ => vec![],
        }
    }

    /// Whether every part has been sent at least once, so only the proof is outstanding.
    pub fn awaiting_proof(&self) -> bool {
        self.unsent == 0 && !self.done && !self.canceled
    }

    /// While [`awaiting_proof`](Self::awaiting_proof), ask the receiver to re-send its
    /// proof: a `CACHE_REQUEST` naming the full hash of the proof packet a correct transfer
    /// produces, as RNS asks its peer's packet cache. At most [`PROOF_CACHE_REQUESTS`]
    /// times, renewed whenever another part request arrives; `None` once spent.
    pub fn cache_request(&mut self) -> Option<Packet> {
        if !self.awaiting_proof() || self.cache_requests_left == 0 {
            return None;
        }
        self.cache_requests_left -= 1;
        let expected = self
            .link
            .resource_proof_packet(&self.out.resource_hash(), &self.out.expected_proof());
        Some(
            self.link
                .framed_packet(CTX_CACHE_REQUEST, expected.full_hash().to_vec()),
        )
    }

    /// Cancel this transfer: the sealed initiator cancel (`RESOURCE_ICL`) to send, or
    /// `None` if it already concluded. Nothing further is served.
    pub fn cancel(&mut self, iv: &[u8; IV_LEN]) -> Option<Packet> {
        if self.done || self.canceled {
            return None;
        }
        self.canceled = true;
        Some(cancel_packet(
            &self.link,
            CTX_RESOURCE_ICL,
            &self.out.resource_hash(),
            iv,
        ))
    }

    /// Whether the receiver has proved receipt.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Whether a valid receiver request has begun the part exchange.
    pub fn has_started(&self) -> bool {
        self.started
    }

    /// Number of requested parts matched and emitted so far.
    pub fn served_parts(&self) -> usize {
        self.served_parts
    }

    /// Whether the transfer was canceled: by the receiver's cancel or rejection naming
    /// it, or by [`cancel`](Self::cancel).
    pub fn is_canceled(&self) -> bool {
        self.canceled
    }

    /// The hash identifying this resource on the wire.
    pub fn resource_hash(&self) -> [u8; 32] {
        self.out.resource_hash()
    }
}

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
    /// The most parts this receiver will accept for one segment.
    ///
    /// A sender chooses the advertised part count, so without this a peer decides how much
    /// memory this node spends on reassembly. The desktop default covers a single-segment
    /// resource; a board sets it far lower.
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

    /// The part ceiling this receiver enforces.
    pub fn max_parts(&self) -> usize {
        self.max_parts
    }

    /// Why this receiver failed, if it has: [`Error::MultiSegmentResource`] for an offer
    /// it cannot reassemble whole, [`Error::CapacityExceeded`] for one past its part or
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
                    return self.next_requests(&mut iv);
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
        // segment. This receiver reassembles a single segment, so accepting the first
        // would hand back its data as if it were the whole resource.
        if adv.l > 1 {
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
        // An advertisement past this receiver's part ceiling is refused outright: the
        // sender chose that number, so honouring it would let a peer decide how much
        // memory this node spends.
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
                // A bz2 bomb, or simply more than this node will hold; or a body that does
                // not open or match its hash. Fail the transfer, release the parts, and
                // tell the sender to stop.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;
    use crate::link::{LinkMode, LinkTrailer, PendingLink, accept};
    use crate::lossy::LossModel;

    /// An established link between a sender side and a receiver side.
    fn link_pair_with_mtu(mtu: u32) -> (Link, Link) {
        let server = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu,
        };
        let dest = DestinationName::new("retinue", ["res"]).destination_hash(server.public());
        let (pending, request) = PendingLink::open(dest, *server.public(), &[0x33; 64], trailer);
        let (recv_link, proof) = accept(&request, &server, &[0x99; 64], trailer).unwrap();
        let send_link = pending.prove(&proof).unwrap();
        (send_link, recv_link)
    }

    fn link_pair() -> (Link, Link) {
        link_pair_with_mtu(500)
    }

    fn iv_gen() -> impl FnMut() -> [u8; IV_LEN] {
        let mut n: u64 = 0;
        move || {
            n += 1;
            let mut v = [0u8; IV_LEN];
            v[..8].copy_from_slice(&n.to_le_bytes());
            v
        }
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len as u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
            .collect()
    }

    fn sha256_counter_payload(len: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(len);
        let mut counter = 0_u64;
        while data.len() < len {
            data.extend_from_slice(&crate::hash::full_hash(&counter.to_be_bytes()));
            counter += 1;
        }
        data.truncate(len);
        data
    }

    /// A clean transfer with no loss: advertise, request, serve, prove — end to end.
    #[test]
    fn transfers_a_small_resource() {
        let (send_link, recv_link) = link_pair();
        let data = payload(3000);
        let mut ivg = iv_gen();
        let mut sender =
            ResourceSender::publish(send_link, &data, [0xAB, 0xCD, 0xEF, 0x01], &ivg());
        let mut receiver = ResourceReceiver::new(recv_link);

        // The receiver gets the advertisement and drives to completion.
        let mut to_receiver = vec![sender.advertisement(&ivg())];
        let mut to_sender: Vec<Packet> = Vec::new();
        for _ in 0..100 {
            for pkt in core::mem::take(&mut to_receiver) {
                to_sender.extend(receiver.on_packet(&pkt, &mut ivg));
            }
            for pkt in core::mem::take(&mut to_sender) {
                to_receiver.extend(sender.on_packet(&pkt, &mut ivg));
            }
            if sender.is_done() && receiver.is_complete() {
                break;
            }
        }
        assert!(sender.is_done(), "sender saw the proof");
        assert_eq!(receiver.data(), Some(data.as_slice()), "payload recovered");
    }

    #[cfg(feature = "compression")]
    #[test]
    fn sender_compresses_when_the_encoded_body_is_smaller() {
        let (send_link, recv_link) = link_pair();
        let data = vec![b'a'; 128 * 1024];
        let mut ivg = iv_gen();
        let mut sender =
            ResourceSender::publish(send_link, &data, [0xAB, 0xCD, 0xEF, 0x01], &ivg());
        let advertisement = sender.advertisement(&ivg());
        let plain = recv_link.decrypt(&advertisement).unwrap();
        let advertised = Advertisement::parse(&plain).unwrap();
        assert_ne!(advertised.flags & crate::resource::FLAG_COMPRESSED, 0);
        assert!(advertised.transfer_size < data.len() as u64);

        let mut receiver = ResourceReceiver::new(recv_link);
        let mut to_receiver = vec![advertisement];
        let mut to_sender: Vec<Packet> = Vec::new();
        for _ in 0..100 {
            for packet in core::mem::take(&mut to_receiver) {
                to_sender.extend(receiver.on_packet(&packet, &mut ivg));
            }
            for packet in core::mem::take(&mut to_sender) {
                to_receiver.extend(sender.on_packet(&packet, &mut ivg));
            }
            if sender.is_done() && receiver.is_complete() {
                break;
            }
        }
        assert!(sender.is_done(), "sender saw the proof");
        assert_eq!(receiver.data(), Some(data.as_slice()), "payload recovered");
    }

    #[cfg(feature = "compression")]
    #[test]
    fn sender_keeps_an_incompressible_body_plain() {
        let (send_link, recv_link) = link_pair();
        let data = sha256_counter_payload(128 * 1024);
        let mut ivg = iv_gen();
        let sender = ResourceSender::publish(send_link, &data, [0xA1, 0xB2, 0xC3, 0xD4], &ivg());
        let advertisement = sender.advertisement(&ivg());
        let plain = recv_link.decrypt(&advertisement).unwrap();
        let advertised = Advertisement::parse(&plain).unwrap();
        assert_eq!(advertised.flags & crate::resource::FLAG_COMPRESSED, 0);
        assert!(advertised.transfer_size > data.len() as u64);
    }

    #[test]
    fn negotiated_mtu_bounds_resource_frames() {
        let (send_link, recv_link) = link_pair_with_mtu(255);
        let data = payload(4_096);
        let mut ivg = iv_gen();
        let mut sender =
            ResourceSender::publish(send_link, &data, [0x10, 0x20, 0x30, 0x40], &ivg());
        let mut receiver = ResourceReceiver::with_request_window(recv_link, 1);
        let mut to_receiver = vec![sender.advertisement(&ivg())];
        let mut to_sender = Vec::new();

        for _ in 0..500 {
            for packet in core::mem::take(&mut to_receiver) {
                assert!(packet.encoded_len() <= 255);
                to_sender.extend(receiver.on_packet(&packet, &mut ivg));
            }
            for packet in core::mem::take(&mut to_sender) {
                assert!(packet.encoded_len() <= 255);
                let replies = sender.on_packet(&packet, &mut ivg);
                assert!(replies.len() <= 1, "one-part request window");
                to_receiver.extend(replies);
            }
            if sender.is_done() && receiver.is_complete() {
                break;
            }
        }

        assert!(sender.has_started());
        assert!(sender.is_done());
        assert_eq!(receiver.data(), Some(data.as_slice()));
    }

    /// A multi-part transfer over a lossy pipe, exercising retransmission of the
    /// advertisement, requests, parts, and the proof, and the HMU path for a large hashmap.
    #[test]
    fn transfers_a_large_resource_over_loss() {
        let (send_link, recv_link) = link_pair();
        // Big enough to need many parts and stream the hashmap over more than one HMU.
        let data = payload(45_000);
        let mut ivg = iv_gen();
        let mut sender =
            ResourceSender::publish(send_link, &data, [0x01, 0x02, 0x03, 0x04], &ivg());
        let mut receiver = ResourceReceiver::new(recv_link);

        let mut fwd = LossModel::new(7).drop_per_mille(150).max_delay_ms(3);
        let mut bwd = LossModel::new(0x5151).drop_per_mille(150).max_delay_ms(3);
        let mut to_receiver: Vec<(u64, Packet)> = Vec::new();
        let mut to_sender: Vec<(u64, Packet)> = Vec::new();

        for now in 0..400_000u64 {
            // Retransmit on a tick: the sender re-advertises until acked; the receiver
            // re-requests what it still lacks.
            if now % 50 == 0 {
                if !sender.is_done() {
                    let adv = sender.advertisement(&ivg());
                    if !fwd.should_drop() {
                        to_receiver.push((now + 1 + fwd.delay_ms(), adv));
                    }
                }
                for pkt in receiver.retransmit(&mut ivg) {
                    if !bwd.should_drop() {
                        to_sender.push((now + 1 + bwd.delay_ms(), pkt));
                    }
                }
            }
            let mut still = Vec::new();
            for (t, pkt) in core::mem::take(&mut to_receiver) {
                if t <= now {
                    for out in receiver.on_packet(&pkt, &mut ivg) {
                        if !bwd.should_drop() {
                            to_sender.push((now + 1 + bwd.delay_ms(), out));
                        }
                    }
                } else {
                    still.push((t, pkt));
                }
            }
            to_receiver = still;
            let mut still = Vec::new();
            for (t, pkt) in core::mem::take(&mut to_sender) {
                if t <= now {
                    for out in sender.on_packet(&pkt, &mut ivg) {
                        if !fwd.should_drop() {
                            to_receiver.push((now + 1 + fwd.delay_ms(), out));
                        }
                    }
                } else {
                    still.push((t, pkt));
                }
            }
            to_sender = still;
            if sender.is_done() && receiver.is_complete() {
                break;
            }
        }
        assert!(sender.is_done(), "sender saw the proof over loss");
        assert_eq!(
            receiver.data(),
            Some(data.as_slice()),
            "large payload recovered exactly over loss"
        );
    }

    /// Deliver `packets` to one side, collecting what it answers.
    fn deliver(packets: Vec<Packet>, side: impl FnMut(&Packet) -> Vec<Packet>) -> Vec<Packet> {
        packets.iter().flat_map(side).collect()
    }

    /// A cancel counts only if it decrypts on the link and names the resource in progress,
    /// as RNS matches one. An unsealed cancel or one naming another resource is ignored.
    #[test]
    fn cancels_are_sealed_and_matched_by_resource_hash() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let mut sender =
            ResourceSender::publish(send_link.clone(), &payload(3000), [1, 2, 3, 4], &ivg());
        let mut receiver = ResourceReceiver::new(recv_link.clone());
        let hash = sender.resource_hash();
        assert!(
            !receiver
                .on_packet(&sender.advertisement(&ivg()), &mut ivg)
                .is_empty()
        );

        // The receiver cancels the sender.
        let framed = recv_link.framed_packet(CTX_RESOURCE_RCL, hash.to_vec());
        sender.on_packet(&framed, &mut ivg);
        assert!(
            !sender.is_canceled(),
            "an unsealed cancel is not the receiver's"
        );
        let other = recv_link.sealed_packet(CTX_RESOURCE_RCL, &[0x11; 32], &ivg());
        sender.on_packet(&other, &mut ivg);
        assert!(!sender.is_canceled(), "a cancel for another resource");
        let real = recv_link.sealed_packet(CTX_RESOURCE_RCL, &hash, &ivg());
        sender.on_packet(&real, &mut ivg);
        assert!(sender.is_canceled());

        // The initiator cancels the receiver.
        let framed = send_link.framed_packet(CTX_RESOURCE_ICL, hash.to_vec());
        receiver.on_packet(&framed, &mut ivg);
        assert!(
            !receiver.is_canceled(),
            "an unsealed cancel is not the sender's"
        );
        let other = send_link.sealed_packet(CTX_RESOURCE_ICL, &[0x22; 32], &ivg());
        receiver.on_packet(&other, &mut ivg);
        assert!(!receiver.is_canceled(), "a cancel for another resource");
        let real = send_link.sealed_packet(CTX_RESOURCE_ICL, &hash, &ivg());
        receiver.on_packet(&real, &mut ivg);
        assert!(receiver.is_canceled());
        assert!(receiver.retransmit(&mut ivg).is_empty());
    }

    /// Cancelling locally sends the sealed cancel RNS reads: ICL from the publisher, RCL
    /// from the receiver, each naming the resource, and the far side stops on it.
    #[test]
    fn a_local_cancel_tells_the_peer() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let mut sender =
            ResourceSender::publish(send_link.clone(), &payload(3000), [1, 2, 3, 4], &ivg());
        let mut receiver = ResourceReceiver::new(recv_link.clone());
        assert!(receiver.cancel(&ivg()).is_none(), "nothing to cancel yet");
        receiver.on_packet(&sender.advertisement(&ivg()), &mut ivg);

        let icl = sender.cancel(&ivg()).expect("a running publish cancels");
        assert_eq!(icl.context, CTX_RESOURCE_ICL);
        assert_eq!(recv_link.decrypt(&icl).unwrap(), sender.resource_hash());
        assert!(sender.cancel(&ivg()).is_none(), "once");
        receiver.on_packet(&icl, &mut ivg);
        assert!(receiver.is_canceled());

        let mut receiver = ResourceReceiver::new(recv_link);
        let mut sender = ResourceSender::publish(send_link.clone(), &payload(3000), [5; 4], &ivg());
        receiver.on_packet(&sender.advertisement(&ivg()), &mut ivg);
        let rcl = receiver.cancel(&ivg()).expect("a running receive cancels");
        assert_eq!(rcl.context, CTX_RESOURCE_RCL);
        assert_eq!(send_link.decrypt(&rcl).unwrap(), sender.resource_hash());
        assert!(receiver.retransmit(&mut ivg).is_empty());
        sender.on_packet(&rcl, &mut ivg);
        assert!(sender.is_canceled());
    }

    /// An accept hook sees the advertisement before any part is requested. An offer it
    /// refuses is rejected on the wire, and the publisher stops on the rejection.
    #[test]
    fn an_offer_the_accept_hook_refuses_is_rejected() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let data = payload(5000);
        let mut sender = ResourceSender::publish(send_link.clone(), &data, [7; 4], &ivg());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut receiver = ResourceReceiver::new(recv_link).with_accept({
            let seen = seen.clone();
            move |advertisement| {
                *seen.lock().unwrap() = Some(advertisement.data_size);
                false
            }
        });
        let replies = receiver.on_packet(&sender.advertisement(&ivg()), &mut ivg);
        assert_eq!(
            *seen.lock().unwrap(),
            Some(data.len() as u64),
            "the hook saw the size"
        );
        assert_eq!(replies.len(), 1, "a rejection, no part request");
        assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
        assert_eq!(receiver.failure(), Some(Error::ResourceRejected));
        deliver(replies, |packet| sender.on_packet(packet, &mut ivg));
        assert!(sender.is_canceled());
    }

    /// A receiver bounded by a data size, as a request bounds its response, rejects a
    /// larger offer and accepts one that fits.
    #[test]
    fn an_offer_past_the_size_limit_is_rejected() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let data = payload(5000);
        let sender = ResourceSender::publish(send_link, &data, [7; 4], &ivg());
        let mut small = ResourceReceiver::new(recv_link.clone()).with_max_data_size(4999);
        let replies = small.on_packet(&sender.advertisement(&ivg()), &mut ivg);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
        assert_eq!(small.failure(), Some(Error::CapacityExceeded));

        let mut fits = ResourceReceiver::new(recv_link).with_max_data_size(5000);
        let replies = fits.on_packet(&sender.advertisement(&ivg()), &mut ivg);
        assert_eq!(replies[0].context, CTX_RESOURCE_REQ);
        assert_eq!(fits.failure(), None);
    }

    /// An offer past the part ceiling is rejected on the wire rather than ignored, so the
    /// publisher stops instead of re-advertising until it times out.
    #[test]
    fn an_offer_past_the_part_ceiling_is_rejected() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let data = sha256_counter_payload(5000);
        let mut sender = ResourceSender::publish(send_link, &data, [7; 4], &ivg());
        let mut receiver = ResourceReceiver::with_limits(recv_link, 4, 2);
        let replies = receiver.on_packet(&sender.advertisement(&ivg()), &mut ivg);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
        assert_eq!(receiver.failure(), Some(Error::CapacityExceeded));
        deliver(replies, |packet| sender.on_packet(packet, &mut ivg));
        assert!(sender.is_canceled());
    }

    /// A body that does not open under the link key fails the transfer with a cancel,
    /// rather than leaving the publisher waiting for a proof that never comes.
    #[test]
    fn a_corrupt_body_fails_with_a_cancel() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let data = payload(2000);
        let random_hash = [9, 9, 9, 9];
        // Not a token this link sealed: every part matches its map hash, and the whole
        // does not open here.
        let token = sha256_counter_payload(2048);
        let out = Outgoing::new(&data, &token, random_hash, false);
        let advertisement =
            send_link.sealed_packet(CTX_RESOURCE_ADV, &out.advertisement().pack(), &ivg());
        let mut receiver = ResourceReceiver::new(recv_link);
        let request = receiver.on_packet(&advertisement, &mut ivg);
        let request = parse_request(&send_link.decrypt(&request[0]).unwrap()).unwrap();
        let replies: Vec<Packet> = out
            .serve(&request)
            .into_iter()
            .flat_map(|part| {
                receiver.on_packet(&send_link.framed_packet(CTX_RESOURCE, part), &mut ivg)
            })
            .collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
        assert_eq!(send_link.decrypt(&replies[0]).unwrap(), out.resource_hash());
        assert_eq!(receiver.failure(), Some(Error::ResourceCorrupt));
        assert_eq!(receiver.data(), None);
    }

    /// Metadata rides in front of the data, flagged in the advertisement, and comes out
    /// separately; the proof covers both, as RNS's does.
    #[test]
    fn metadata_round_trips_beside_the_data() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let data = payload(9000);
        // msgpack {"name": "x.bin"}
        let metadata = b"\x81\xa4name\xa5x.bin";
        let mut sender =
            ResourceSender::publish_with_metadata(send_link, &data, metadata, [3, 3, 3, 3], &ivg())
                .unwrap();
        let advertisement = sender.advertisement(&ivg());
        let advertised = Advertisement::parse(&recv_link.decrypt(&advertisement).unwrap()).unwrap();
        assert!(advertised.has_metadata());
        assert_eq!(
            advertised.data_size,
            (3 + metadata.len() + data.len()) as u64
        );

        let mut receiver = ResourceReceiver::new(recv_link);
        let mut to_receiver = vec![advertisement];
        for _ in 0..100 {
            let to_sender = deliver(core::mem::take(&mut to_receiver), |packet| {
                receiver.on_packet(packet, &mut ivg)
            });
            to_receiver = deliver(to_sender, |packet| sender.on_packet(packet, &mut ivg));
            if sender.is_done() {
                break;
            }
        }
        assert!(sender.is_done(), "the proof over metadata and data matched");
        assert_eq!(receiver.data(), Some(data.as_slice()));
        assert_eq!(receiver.metadata(), Some(&metadata[..]));
    }

    /// A publisher that has sent every part and heard no proof asks for it with a cache
    /// request naming the expected proof packet's full hash, at most three times; the
    /// receiver's kept proof is exactly that packet.
    #[test]
    fn a_lost_proof_is_asked_for_by_hash() {
        let mut ivg = iv_gen();
        let (mut sender, receiver, proof) = transfer_until_proof(&payload(3000), &mut ivg);
        assert!(sender.awaiting_proof());
        let request = sender.cache_request().expect("a cache request");
        assert_eq!(request.context, CTX_CACHE_REQUEST);
        assert_eq!(request.payload, proof.full_hash());
        assert_eq!(receiver.proof_packet(), Some(proof.clone()));
        assert!(sender.cache_request().is_some());
        assert!(sender.cache_request().is_some());
        assert!(sender.cache_request().is_none(), "three at most");
        sender.on_packet(&proof, &mut ivg);
        assert!(sender.is_done());
        assert!(!sender.awaiting_proof());
    }

    /// A proof that names another resource does not complete the publisher.
    #[test]
    fn a_proof_for_another_resource_does_not_complete() {
        let mut ivg = iv_gen();
        let (mut sender, _, proof) = transfer_until_proof(&payload(3000), &mut ivg);
        let mut forged = proof.clone();
        forged.payload[0] ^= 1;
        sender.on_packet(&forged, &mut ivg);
        assert!(!sender.is_done());
        sender.on_packet(&proof, &mut ivg);
        assert!(sender.is_done());
    }

    /// Drive a clean transfer to the receiver's proof, returning the sender (not yet shown
    /// the proof), the receiver, and the proof packet the receiver emitted.
    fn transfer_until_proof(
        data: &[u8],
        ivg: &mut impl FnMut() -> [u8; IV_LEN],
    ) -> (ResourceSender, ResourceReceiver, Packet) {
        let (send_link, recv_link) = link_pair();
        let mut sender = ResourceSender::publish(send_link, data, [9, 8, 7, 6], &ivg());
        let mut receiver = ResourceReceiver::new(recv_link);
        let mut to_receiver = vec![sender.advertisement(&ivg())];
        for _ in 0..100 {
            let mut to_sender = Vec::new();
            for packet in core::mem::take(&mut to_receiver) {
                to_sender.extend(receiver.on_packet(&packet, &mut *ivg));
            }
            if let Some(index) = to_sender.iter().position(|p| p.context == CTX_RESOURCE_PRF) {
                return (sender, receiver, to_sender.swap_remove(index));
            }
            for packet in to_sender {
                to_receiver.extend(sender.on_packet(&packet, &mut *ivg));
            }
        }
        panic!("the receiver never proved");
    }

    /// RNS concludes a resource only on a PROOF-type packet (`Link.receive` dispatches
    /// RESOURCE_PRF under `PacketType::Proof`), so the receipt and its retransmissions must
    /// be one, unencrypted, carrying `resource_hash || proof`.
    #[test]
    fn the_resource_proof_is_a_proof_type_packet() {
        let mut ivg = iv_gen();
        let data = payload(3000);
        let (mut sender, mut receiver, proof) = transfer_until_proof(&data, &mut ivg);
        assert_eq!(proof.packet_type, crate::packet::PacketType::Proof);
        let (hash, _) = parse_proof(&proof.payload).expect("hash || proof, in the clear");
        assert_eq!(hash, sender.out.resource_hash());

        let replayed = receiver.retransmit(&mut ivg);
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].packet_type, crate::packet::PacketType::Proof);
        assert_eq!(replayed[0].payload, proof.payload);

        sender.on_packet(&proof, &mut ivg);
        assert!(
            sender.is_done(),
            "the PROOF-type receipt completes the sender"
        );
    }

    /// For one release a sender still accepts the DATA-type proof older retinue sent.
    #[test]
    fn a_sender_still_accepts_the_legacy_data_type_proof() {
        let mut ivg = iv_gen();
        let data = payload(1200);
        let (mut sender, _, proof) = transfer_until_proof(&data, &mut ivg);
        let legacy = sender
            .link
            .framed_packet(CTX_RESOURCE_PRF, proof.payload.clone());
        assert_eq!(legacy.packet_type, crate::packet::PacketType::Data);
        sender.on_packet(&legacy, &mut ivg);
        assert!(sender.is_done());
    }

    /// A multi-segment advertisement (`l > 1`) is refused with a sealed receiver cancel
    /// naming the resource, and the receiver never yields the first segment as if it were
    /// the whole resource, even when every part of that segment arrives.
    #[test]
    fn a_multi_segment_advertisement_is_refused_and_never_yields_data() {
        let (send_link, recv_link) = link_pair();
        let mut ivg = iv_gen();
        let segment = payload(2000);
        let random_hash = [0x51, 0x52, 0x53, 0x54];
        let token = send_link.seal(&content(&segment, &random_hash), &ivg());
        let out = Outgoing::new(&segment, &token, random_hash, false).with_segment(
            1,
            2,
            4000,
            crate::resource::resource_hash(&segment, &random_hash),
        );
        let advertised = out.advertisement();
        assert_eq!(advertised.l, 2);
        let advertisement = send_link.sealed_packet(CTX_RESOURCE_ADV, &advertised.pack(), &ivg());

        let mut receiver = ResourceReceiver::new(recv_link.clone());
        let replies = receiver.on_packet(&advertisement, &mut ivg);
        assert_eq!(replies.len(), 1, "one refusal, no part request");
        assert_eq!(replies[0].context, CTX_RESOURCE_RCL);
        assert_eq!(
            send_link.decrypt(&replies[0]).unwrap(),
            out.resource_hash().to_vec(),
            "the cancel is sealed and names the resource, as RNS reads it"
        );
        assert_eq!(receiver.failure(), Some(Error::MultiSegmentResource));

        // A re-sent advertisement is refused again; every part of the segment is ignored.
        let again = receiver.on_packet(&advertisement, &mut ivg);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].context, CTX_RESOURCE_RCL);
        let wanted = advertised.hashmap.as_chunks::<4>().0.to_vec();
        assert_eq!(
            wanted.len(),
            out.total_parts(),
            "the advert names every part"
        );
        let all = crate::resource::build_request(&out.resource_hash(), &wanted);
        let request = crate::resource::parse_request(&all).unwrap();
        for part in out.serve(&request) {
            let packet = send_link.framed_packet(CTX_RESOURCE, part);
            assert!(receiver.on_packet(&packet, &mut ivg).is_empty());
        }
        assert!(receiver.retransmit(&mut ivg).is_empty());
        assert!(!receiver.is_complete());
        assert_eq!(receiver.data(), None);
    }

    /// A compressed body that inflates past the receiver's limit fails the transfer with a
    /// typed error and a sealed cancel, rather than being inflated and returned.
    #[cfg(feature = "compression")]
    #[test]
    fn a_body_past_the_decompression_limit_fails_the_transfer() {
        let (send_link, recv_link) = link_pair();
        let data = vec![0_u8; 256 * 1024];
        let mut ivg = iv_gen();
        let mut sender = ResourceSender::publish(send_link.clone(), &data, [3, 1, 4, 1], &ivg());
        let mut receiver = ResourceReceiver::new(recv_link).with_max_decompressed_size(64 * 1024);
        let mut to_receiver = vec![sender.advertisement(&ivg())];
        let mut cancel = None;
        for _ in 0..100 {
            let mut to_sender = Vec::new();
            for packet in core::mem::take(&mut to_receiver) {
                to_sender.extend(receiver.on_packet(&packet, &mut ivg));
            }
            for packet in to_sender {
                assert_ne!(packet.context, CTX_RESOURCE_PRF, "nothing is proved");
                if packet.context == CTX_RESOURCE_RCL {
                    cancel = Some(packet.clone());
                }
                to_receiver.extend(sender.on_packet(&packet, &mut ivg));
            }
            if to_receiver.is_empty() {
                break;
            }
        }
        assert_eq!(receiver.failure(), Some(Error::DecompressionLimit));
        assert_eq!(receiver.data(), None);
        assert!(receiver.retransmit(&mut ivg).is_empty());
        let cancel = cancel.expect("the sender is told to stop");
        assert_eq!(
            send_link.decrypt(&cancel).unwrap(),
            sender.out.resource_hash().to_vec()
        );
        assert!(sender.is_canceled());
    }
}
