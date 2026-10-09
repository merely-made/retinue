//! The publishing half of a resource transfer.

use alloc::vec;
use alloc::vec::Vec;

use super::PROOF_CACHE_REQUESTS;
use super::cancel::{cancel_packet, names_resource};
use crate::Error;
use crate::link::{
    CTX_CACHE_REQUEST, CTX_RESOURCE, CTX_RESOURCE_ADV, CTX_RESOURCE_HMU, CTX_RESOURCE_ICL,
    CTX_RESOURCE_PRF, CTX_RESOURCE_RCL, CTX_RESOURCE_REQ, Link,
};
use crate::packet::Packet;
#[cfg(feature = "compression")]
use crate::resource::compress;
use crate::resource::{
    Outgoing, RANDOM_HASH_LEN, SDU, content, pack_metadata, parse_proof, parse_request,
};
use crate::token::IV_LEN;

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
    /// link MTU.
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

/// Seal `data` into an [`Outgoing`] whose parts fit `link`'s MTU, bz2-compressing it first
/// when `compress` is set and that shrinks it.
pub(super) fn outgoing(
    link: &Link,
    data: &[u8],
    random_hash: [u8; RANDOM_HASH_LEN],
    iv: &[u8; IV_LEN],
    try_compress: bool,
) -> Outgoing {
    #[cfg(feature = "compression")]
    let encoded = try_compress
        .then(|| compress(data))
        .filter(|encoded| encoded.len() < data.len());
    #[cfg(not(feature = "compression"))]
    let encoded: Option<Vec<u8>> = {
        let _ = try_compress;
        None
    };
    let compressed = encoded.is_some();
    let transfer = content(encoded.as_deref().unwrap_or(data), &random_hash);
    drop(encoded);
    let token = link.seal(&transfer, iv);
    drop(transfer);
    let part_size = (link.mtu() as usize)
        .saturating_sub(crate::packet::HEADER_MIN_LEN)
        .clamp(1, SDU);
    Outgoing::from_token(data, token, random_hash, compressed, part_size)
}
