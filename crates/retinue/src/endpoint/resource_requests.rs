//! Requests and responses on a `ResourceSession`.

use alloc::vec::Vec;

use std::io;
use std::sync::Arc;

use crate::hash::AddressHash;
use crate::identity::Identity;
use crate::link::Inbound;
use crate::request::{Request, Response};
use crate::resource::RANDOM_HASH_LEN;
use crate::resource_transfer::{ResourceReceiver, ResourceSender};

use super::entropy::{fill_random, next_iv};
use super::resource_session::{
    PayloadMode, ResourceSession, keep_resource_proof, resource_receive_ended,
};
use super::stream::write_chunk_for_mtu;

/// One request received over a resource-capable link.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedRequest {
    /// The decoded request body.
    pub request: Request,
    /// Hash of the encrypted request packet, echoed by the response.
    pub request_id: AddressHash,
    /// Identity proven by a preceding link IDENTIFY, when present.
    pub peer: Option<Identity>,
}

/// One decrypted request packet before an application interprets its
/// MessagePack value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedRawRequest {
    /// Complete decrypted request structure.
    pub packed: Vec<u8>,
    /// Hash of the encrypted request packet, echoed by the response.
    pub request_id: AddressHash,
    /// Identity proven by a preceding link IDENTIFY, when present.
    pub peer: Option<Identity>,
}

/// One decrypted response before an application interprets its MessagePack
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedRawResponse {
    /// Complete decrypted response structure.
    pub packed: Vec<u8>,
    /// Request id read from the first response item.
    pub request_id: AddressHash,
    /// Packed (msgpack) metadata of a response Resource that carried some, RNS's file
    /// response: `packed` then holds the file's bytes, and `request_id` is the advertised one.
    pub metadata: Option<Vec<u8>>,
}

impl ResourceSession {
    async fn publish_response_value(
        &mut self,
        request_id: AddressHash,
        packed_value: &[u8],
    ) -> io::Result<()> {
        let mut random_hash = [0_u8; RANDOM_HASH_LEN];
        fill_random(&mut random_hash);
        let sender = ResourceSender::respond(
            self.link.clone(),
            packed_value,
            *request_id.as_bytes(),
            random_hash,
            &next_iv(),
        );
        self.publish_sender(sender).await
    }

    /// The peer identity proven by an IDENTIFY on this link, if the sender sent one.
    ///
    /// Stronger than an announce: the peer on *this* link signed it. It says nothing about
    /// who a payload claims to be from, which the caller must still check.
    pub fn identified_peer(&self) -> Option<Identity> {
        self.identified_peer
    }

    /// Wait for one request packet on this link.
    pub async fn receive_request(&mut self) -> io::Result<ReceivedRequest> {
        let raw = self.receive_raw_request().await?;
        let request = Request::unpack(&raw.packed).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid byte request payload")
        })?;
        Ok(ReceivedRequest {
            request,
            request_id: raw.request_id,
            peer: raw.peer,
        })
    }

    /// Wait for one request and retain its complete decrypted MessagePack.
    ///
    /// RNS permits the request's third item to be an application value rather
    /// than a binary blob. Consumers with their own grammar use this method;
    /// byte-oriented requests can use [`receive_request`](Self::receive_request).
    pub async fn receive_raw_request(&mut self) -> io::Result<ReceivedRawRequest> {
        let link = self.link.clone();
        let packets = &mut self.packets;
        let mut peer = self.identified_peer;
        let receive = async move {
            loop {
                let packet = packets.recv().await.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "request link closed")
                })?;
                if let Some(identity) = link.read_identify(&packet) {
                    peer = Some(identity);
                    continue;
                }
                match link.receive(&packet) {
                    Some(Inbound::Request(bytes)) => {
                        return Ok(ReceivedRawRequest {
                            packed: bytes,
                            request_id: packet.hash(),
                            peer,
                        });
                    }
                    Some(Inbound::Close) => {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "request link closed",
                        ));
                    }
                    _ => {}
                }
            }
        };
        let received = tokio::time::timeout(self.config.timeout, receive)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request receive timed out"))??;
        self.identified_peer = received.peer;
        if let Some(identity) = received.peer {
            self.retain_identified_peer(identity);
        }
        Ok(received)
    }

    /// Send one response to a request received on this session.
    pub fn respond(&self, request_id: AddressHash, data: Vec<u8>) {
        let response = Response::new(request_id, data);
        self.shared.send_on(
            self.iface,
            self.link.response_packet(&response.pack(), &next_iv()),
        );
    }

    /// Send a response whose data is already one MessagePack value.
    pub fn respond_value(&self, request_id: AddressHash, packed_value: &[u8]) {
        let packed = Response::pack_value(request_id, packed_value);
        self.shared
            .send_on(self.iface, self.link.response_packet(&packed, &next_iv()));
    }

    /// Respond with opaque bytes, degrading to a Resource when the complete
    /// response envelope does not fit one encrypted link packet.
    pub async fn respond_auto(
        &mut self,
        request_id: AddressHash,
        data: Vec<u8>,
    ) -> io::Result<PayloadMode> {
        let packed_value = Response::pack_binary_value(&data);
        self.respond_value_auto(request_id, &packed_value).await
    }

    /// Respond with one already-packed MessagePack value, degrading to a
    /// Resource when the complete response envelope does not fit one encrypted
    /// link packet.
    pub async fn respond_value_auto(
        &mut self,
        request_id: AddressHash,
        packed_value: &[u8],
    ) -> io::Result<PayloadMode> {
        let packed = Response::pack_value(request_id, packed_value);
        if packed.len() <= write_chunk_for_mtu(self.link.mtu()) {
            self.shared
                .send_on(self.iface, self.link.response_packet(&packed, &next_iv()));
            Ok(PayloadMode::Data)
        } else {
            self.publish_response_value(request_id, &packed).await?;
            Ok(PayloadMode::Resource)
        }
    }

    /// Identify this endpoint's local identity to the remote link.
    pub fn identify(&self) {
        self.shared.send_on(
            self.iface,
            self.link.identify_packet(&self.shared.identity, &next_iv()),
        );
    }

    /// Send one request and wait for its matching response.
    pub async fn request(&mut self, request: &Request) -> io::Result<Response> {
        let raw = self.request_raw(&request.pack()).await?;
        Response::unpack(&raw.packed).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid byte response payload")
        })
    }

    /// Send one already-packed request and retain the raw matching response.
    pub async fn request_raw(&mut self, packed_request: &[u8]) -> io::Result<ReceivedRawResponse> {
        self.request_raw_with_limit(packed_request, None).await
    }

    /// [`request_raw`](Self::request_raw), refusing a response larger than
    /// `max_response_size` bytes, as RNS's `Link.request(max_response_size=...)` does: a
    /// response Resource advertising more is rejected on the wire before any part is
    /// requested, and the call fails with [`io::ErrorKind::InvalidData`].
    pub async fn request_raw_with_limit(
        &mut self,
        packed_request: &[u8],
        max_response_size: Option<usize>,
    ) -> io::Result<ReceivedRawResponse> {
        // Outgoing request Resources are not implemented.
        if packed_request.len() > write_chunk_for_mtu(self.link.mtu()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request exceeds link packet capacity",
            ));
        }
        let packet = self.link.request_packet(packed_request, &next_iv());
        let request_id = packet.hash();
        self.shared.send_on(self.iface, packet);

        let link = self.link.clone();
        let shared = Arc::clone(&self.shared);
        let iface = self.iface;
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let request_window = self.config.request_window;
        let response_receiver = move || {
            let receiver = ResourceReceiver::with_request_window(link.clone(), request_window);
            match max_response_size {
                Some(max) => receiver.with_max_data_size(max),
                None => receiver,
            }
        };
        let mut receiver = response_receiver();
        let link = self.link.clone();
        let receiving = &mut receiver;
        let receive = async move {
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "request link closed")
                        })?;
                        match link.receive(&packet) {
                            Some(Inbound::Response(bytes)) => {
                                let response_id = Response::request_id(&bytes).map_err(|_| {
                                    io::Error::new(io::ErrorKind::InvalidData, "invalid response envelope")
                                })?;
                                if response_id == request_id {
                                    // RNS sizes a packet response by its packed value, less
                                    // two bytes: the envelope is a fixarray, a bin8 header and
                                    // the 16-byte id.
                                    let size = bytes.len().saturating_sub(1 + 2 + 16 + 2);
                                    if max_response_size.is_some_and(|max| size > max) {
                                        return Err(io::Error::new(
                                            io::ErrorKind::InvalidData,
                                            "response exceeds max_response_size",
                                        ));
                                    }
                                    return Ok(ReceivedRawResponse {
                                        packed: bytes,
                                        request_id: response_id,
                                        metadata: None,
                                    });
                                }
                            }
                            Some(Inbound::Close) => {
                                return Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    "request link closed",
                                ));
                            }
                            _ => {}
                        }
                        for outbound in receiving.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        if receiving.is_complete() {
                            keep_resource_proof(&shared, iface, link.id(), receiving);
                            let advertised_id = receiving.response_request_id().map(AddressHash::from_bytes);
                            let (packed, metadata) = receiving
                                .take_payload()
                                .expect("a completed receiver holds its payload");
                            // A file response (one with metadata) carries the file's bytes,
                            // and only its advertisement names the request.
                            let response_id = match (&metadata, advertised_id) {
                                (Some(_), Some(id)) => id,
                                _ => Response::request_id(&packed).map_err(|_| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "invalid response resource",
                                    )
                                })?,
                            };
                            if advertised_id.is_some_and(|id| id != response_id) {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "response Resource request id mismatch",
                                ));
                            }
                            if response_id == request_id {
                                return Ok(ReceivedRawResponse {
                                    packed,
                                    request_id: response_id,
                                    metadata,
                                });
                            }
                            *receiving = response_receiver();
                        } else {
                            resource_receive_ended(receiving)?;
                        }
                    }
                    _ = interval.tick() => {
                        for outbound in receiving.retransmit(next_iv) {
                            shared.send_on(iface, outbound);
                        }
                    }
                }
            }
        };
        let outcome = tokio::time::timeout(self.config.timeout, receive).await;
        self.settle_receive(&mut receiver, outcome, "response receive timed out")
    }
}
