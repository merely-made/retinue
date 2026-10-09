//! The publishing half of a resource transfer.

use alloc::vec;
use alloc::vec::Vec;

use super::PROOF_CACHE_REQUESTS;
use super::cancel::{cancel_packet, names_resource};
use super::timing::{MAX_ADV_RETRIES, Timing};
use crate::Error;
use crate::hash::AddressHash;
use crate::link::{
    CTX_CACHE_REQUEST, CTX_RESOURCE, CTX_RESOURCE_ADV, CTX_RESOURCE_HMU, CTX_RESOURCE_ICL,
    CTX_RESOURCE_PRF, CTX_RESOURCE_RCL, CTX_RESOURCE_REQ, Link,
};
use crate::packet::Packet;
#[cfg(feature = "compression")]
use crate::resource::compress_after;
use crate::resource::{Outgoing, RANDOM_HASH_LEN, SDU, pack_metadata, parse_proof, parse_request};
use crate::token::{IV_LEN, MAC_LEN};

/// What sealing adds around the content: the IV, a padding block and the HMAC.
const TOKEN_ROOM: usize = IV_LEN + 16 + MAC_LEN;

/// Publishes one resource over a link: advertises it, serves part requests and hashmap
/// updates, and completes when the receiver's proof of receipt arrives.
pub struct ResourceSender {
    pub(super) link: Link,
    pub(super) out: Outgoing,
    hash_window: usize,
    started: bool,
    served_parts: usize,
    /// Which parts have been sent at least once, and how many have not.
    sent: Vec<bool>,
    unsent: usize,
    cache_requests_left: u8,
    /// Hashes of the latest requests served, so a replayed one is not served again
    /// (`Link.py` 1088-1093). RNS keeps every one; a few suffice to catch a duplicate frame.
    served_requests: Vec<AddressHash>,
    done: bool,
    canceled: bool,
    timed_out: bool,
    timing: Timing,
    /// When the advertisement last went out, and how many re-sends remain.
    advertised_at: Option<u64>,
    adv_retries_left: u8,
    /// The transfer's own RTT: advertisement to first request.
    rtt: Option<u64>,
    last_request: u64,
    last_part_sent: u64,
}

/// How many served request hashes a sender remembers.
const SERVED_REQUESTS: usize = 8;

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
        let mut out = outgoing(&link, data, random_hash, iv, true);
        if let Some(request_id) = request_id {
            out = out.with_request_id(request_id);
        }
        if has_metadata {
            out = out.with_metadata();
        }
        Self::from_outgoing(link, out)
    }

    /// A sender for an already-built resource, its advertised hashmap window fitted to the
    /// link MTU. Batches under 74 hashes are retinue's own: RNS always sends 74 and places
    /// an update at `segment * 74`, so only a retinue receiver follows a narrower window.
    pub(super) fn from_outgoing(link: Link, out: Outgoing) -> Self {
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
            served_requests: Vec::new(),
            done: false,
            canceled: false,
            timed_out: false,
            timing: Timing::default(),
            advertised_at: None,
            adv_retries_left: MAX_ADV_RETRIES,
            rtt: None,
            last_request: 0,
            last_part_sent: 0,
        }
    }

    /// Time retransmissions from `timing`; see [`Timing`].
    pub fn with_timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
    }

    /// The advertisement to send at tick `now`, which starts the advertisement timer.
    /// [`poll`](Self::poll) re-sends it while unanswered.
    pub fn advertise(&mut self, now: u64, iv: &[u8; IV_LEN]) -> Packet {
        self.advertised_at = Some(now);
        self.advertisement(iv)
    }

    /// The advertisement packet, sealed.
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
        now: u64,
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
                let hash = packet.hash();
                if req.resource_hash != self.out.resource_hash()
                    || self.served_requests.contains(&hash)
                {
                    return vec![];
                }
                if self.served_requests.len() == SERVED_REQUESTS {
                    self.served_requests.remove(0);
                }
                self.served_requests.push(hash);
                if !self.started {
                    self.started = true;
                    self.rtt = Some(now.saturating_sub(self.advertised_at.unwrap_or(now)));
                }
                self.last_request = now;
                let mut out = Vec::new();
                // Serve every part whose map hash we hold, framed (already encrypted in-token).
                for index in self.out.requested_indices(&req) {
                    let Some(part) = self.out.part(index) else {
                        continue;
                    };
                    out.push(self.link.framed_packet(CTX_RESOURCE, part.to_vec()));
                    self.served_parts += 1;
                    // A byte-identical part elsewhere shares this map hash, and this copy
                    // serves its slot too.
                    for copy in self.out.copies_of(index) {
                        if !core::mem::replace(&mut self.sent[copy], true) {
                            self.unsent -= 1;
                        }
                    }
                }
                // An exhausted request wants the next slice of the hashmap.
                if req.exhausted
                    && let Some(last) = req.last_map_hash
                {
                    let hmu = self.out.hmu_after_with_hash_limit(&last, self.hash_window);
                    out.push(self.link.sealed_packet(CTX_RESOURCE_HMU, &hmu, &iv()));
                }
                if !out.is_empty() {
                    self.last_part_sent = now;
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

    /// Run the sender's watchdog at tick `now` (`Resource.py` 580-600, 651-671): re-send
    /// an unanswered advertisement up to [`MAX_ADV_RETRIES`] times, ask for a missing proof
    /// with a [`cache_request`](Self::cache_request), and once those run out, or requests
    /// stop mid-transfer, cancel with [`timed_out`](Self::timed_out) set.
    pub fn poll(&mut self, now: u64, mut iv: impl FnMut() -> [u8; IV_LEN]) -> Option<Packet> {
        if now < self.deadline()? {
            return None;
        }
        if !self.started {
            if self.adv_retries_left > 0 {
                self.adv_retries_left -= 1;
                return Some(self.advertise(now, &iv()));
            }
        } else if self.awaiting_proof()
            && let Some(request) = self.cache_request()
        {
            self.last_part_sent = now;
            return Some(request);
        }
        self.timed_out = true;
        self.cancel(&iv())
    }

    /// The tick at which [`poll`](Self::poll) next has work, while the transfer runs.
    pub fn deadline(&self) -> Option<u64> {
        if self.done || self.canceled {
            return None;
        }
        let rtt = self.rtt.unwrap_or(self.timing.rtt);
        let (since, wait) = if !self.started {
            (self.advertised_at?, self.timing.advertisement_wait())
        } else if self.awaiting_proof() {
            (self.last_part_sent, self.timing.proof_wait(rtt))
        } else {
            (self.last_request, self.timing.request_wait(rtt))
        };
        Some(since.saturating_add(wait))
    }

    /// Whether this sender gave up on a silent receiver.
    pub fn timed_out(&self) -> bool {
        self.timed_out
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

/// Seal `data` into an [`Outgoing`] whose parts fit `link`'s MTU, bz2-compressing it first
/// when `try_compress` is set and that shrinks it.
pub(super) fn outgoing(
    link: &Link,
    data: &[u8],
    random_hash: [u8; RANDOM_HASH_LEN],
    iv: &[u8; IV_LEN],
    try_compress: bool,
) -> Outgoing {
    // `random_hash || body`, with room to seal it in place.
    let content = |body_len: usize| {
        let mut transfer = Vec::with_capacity(TOKEN_ROOM + RANDOM_HASH_LEN + body_len);
        transfer.extend_from_slice(&random_hash);
        transfer
    };
    let plain = || {
        let mut transfer = content(data.len());
        transfer.extend_from_slice(data);
        transfer
    };
    #[cfg(feature = "compression")]
    let (transfer, compressed) = if try_compress {
        let encoded = compress_after(content(data.len() / 2), data);
        if encoded.len() - RANDOM_HASH_LEN < data.len() {
            (encoded, true)
        } else {
            drop(encoded);
            (plain(), false)
        }
    } else {
        (plain(), false)
    };
    #[cfg(not(feature = "compression"))]
    let (transfer, compressed) = {
        let _ = try_compress;
        (plain(), false)
    };
    let token = link.seal_owned(transfer, iv);
    // The link MTU less `HEADER_MAXSIZE` and `IFAC_MIN_SIZE` (`Resource.py` 343-344), so a
    // part still fits once a relay addresses it. Held to the SDU above MTU 500.
    let part_size = (link.mtu() as usize)
        .saturating_sub(crate::packet::HEADER_MAX_LEN + 1)
        .clamp(1, SDU);
    Outgoing::from_token(data, token, random_hash, compressed, part_size)
}
