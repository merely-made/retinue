//! Whatever arrives next on a `ResourceSession`: a request, a data packet or a Resource.

use alloc::vec::Vec;

use std::io;
use std::time::Duration;

use crate::hash::AddressHash;
use crate::link::Inbound;
use crate::packet::Packet;
use crate::resource::FLAG_RESPONSE;
use crate::resource_transfer::ResourceKind;

use super::entropy::next_iv;
use super::resource_requests::ReceivedRawRequest;
use super::resource_session::{Pace, ResourceSession, resource_receive_ended};

/// One link data packet, kept so the receiver can prove it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedLinkData {
    /// The decrypted payload.
    pub data: Vec<u8>,
    packet: Packet,
}

/// What [`ResourceSession::next_inbound`] delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionInbound {
    /// A request, in a packet or a request Resource.
    Request(ReceivedRawRequest),
    /// One best-effort link data packet.
    Data(ReceivedLinkData),
    /// One complete application Resource.
    Resource(Vec<u8>),
}

impl ResourceSession {
    /// Wait for whatever the peer sends next, as an RNS destination with a packet
    /// callback, request handlers and Resources dispatches one link (an LXMF propagation
    /// node takes `/get` requests and submissions on the same link).
    ///
    /// `idle` is the longest wait with nothing heard; past it the call fails with
    /// [`io::ErrorKind::TimedOut`]. A failed or refused Resource is dropped and the wait
    /// goes on. Requests are bounded as in [`receive_raw_request`]; the accept hook from
    /// [`set_accept`] sees request advertisements too, flagged `FLAG_REQUEST`.
    ///
    /// [`receive_raw_request`]: Self::receive_raw_request
    /// [`set_accept`]: Self::set_accept
    pub async fn next_inbound(&mut self, idle: Duration) -> io::Result<SessionInbound> {
        let max_request = self.max_request_size;
        let make = self.receivers(None, true);
        let new_receiver = move || make().with_filter(|adv| adv.flags & FLAG_RESPONSE == 0);
        let mut receiver = new_receiver();
        let mut pace = Pace::new(idle);
        let mut kept = 0;
        let closed = || io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed");
        loop {
            tokio::select! {
                maybe = self.packets.recv() => {
                    let packet = maybe.ok_or_else(closed)?;
                    pace.heard();
                    if self.responder && let Some(identity) = self.link.read_identify(&packet) {
                        let identity = *self.identified_peer.get_or_insert(identity);
                        self.retain_identified_peer(identity);
                        continue;
                    }
                    match self.link.receive(&packet) {
                        Some(Inbound::Data(data)) => {
                            return Ok(SessionInbound::Data(ReceivedLinkData { data, packet }));
                        }
                        Some(Inbound::Request(bytes))
                            if max_request.is_none_or(|max| bytes.len() <= max) =>
                        {
                            return Ok(SessionInbound::Request(ReceivedRawRequest {
                                packed: bytes,
                                request_id: packet.hash(),
                                peer: self.identified_peer,
                            }));
                        }
                        Some(Inbound::Close) => return Err(closed()),
                        _ => {}
                    }
                    match self.on_resource_packet(&mut receiver, &packet, &pace, &mut kept) {
                        Ok(true) => {
                            let kind = receiver.kind();
                            let (data, metadata) = receiver
                                .take_payload()
                                .expect("a completed receiver holds its payload");
                            receiver = new_receiver();
                            kept = 0;
                            match kind {
                                Some(ResourceKind::Request(_)) => {
                                    if max_request.is_some_and(|max| data.len() > max) {
                                        continue;
                                    }
                                    return Ok(SessionInbound::Request(ReceivedRawRequest {
                                        request_id: AddressHash::of(&data),
                                        packed: data,
                                        peer: self.identified_peer,
                                    }));
                                }
                                _ => {
                                    self.set_metadata(metadata);
                                    return Ok(SessionInbound::Resource(data));
                                }
                            }
                        }
                        Ok(false) => {}
                        Err(_) => {
                            receiver = new_receiver();
                            kept = 0;
                        }
                    }
                }
                _ = tokio::time::sleep_until(pace.wake(receiver.deadline())) => {
                    if pace.idle() {
                        if let Some(cancel) = receiver.cancel(&next_iv()) {
                            self.shared.send_on(self.iface, cancel);
                        }
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "link idle"));
                    }
                    for outbound in receiver.poll(pace.now(), next_iv) {
                        self.shared.send_on(self.iface, outbound);
                    }
                    if resource_receive_ended(&receiver).is_err() {
                        self.keep_carry(&receiver);
                        receiver = new_receiver();
                        kept = 0;
                    }
                }
            }
        }
    }

    /// Prove a data packet received on this session, as RNS's `packet.prove()` does.
    pub fn prove(&self, received: &ReceivedLinkData) {
        self.shared
            .send_on(self.iface, self.link.prove_packet(&received.packet));
    }

    /// Send one best-effort data packet on this session's link.
    pub fn send_data(&self, data: &[u8]) {
        self.shared
            .send_on(self.iface, self.link.data_packet(data, &next_iv()));
    }
}
