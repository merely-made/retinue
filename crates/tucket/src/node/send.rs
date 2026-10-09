//! Composing outbound frames.

use alloc::{string::String, vec::Vec};

use super::{CapacityError, MAX_TEXT_BYTES, Node, PendingText, TextAttempt, TextRetryPolicy};
use crate::advert::{Advert, AdvertData};
use crate::message::{TextMessage, encode_ack};
use crate::packet::{Packet, ROUTE_DIRECT, ROUTE_FLOOD, payload_type};

impl Node {
    /// Begin a caller-timed private text send. Returns `None` until the peer's
    /// advert has supplied its public key.
    pub fn try_begin_text(
        &self,
        to: u8,
        timestamp: u32,
        text: impl AsRef<str>,
        policy: TextRetryPolicy,
    ) -> Result<PendingText, CapacityError> {
        self.contacts
            .iter()
            .find(|(key, _)| *key == to)
            .ok_or(CapacityError::UnknownContact)?;
        if !(1..=4).contains(&policy.attempts) {
            return Err(CapacityError::InvalidRetryPolicy);
        }
        let text = text.as_ref();
        if text.len() > MAX_TEXT_BYTES {
            return Err(CapacityError::TextTooLong);
        }
        Ok(PendingText {
            to,
            timestamp,
            // Copy only after validating the borrowed input's wire bound.
            text: String::from(text),
            policy,
            next_attempt: 0,
            expected_acks: [[0; 4]; 4],
            expected_ack_count: 0,
            complete: false,
        })
    }

    /// Compatibility wrapper for callers that only need success or refusal.
    pub fn begin_text(
        &self,
        to: u8,
        timestamp: u32,
        text: impl AsRef<str>,
        policy: TextRetryPolicy,
    ) -> Option<PendingText> {
        self.try_begin_text(to, timestamp, text, policy).ok()
    }

    /// Produce the next numbered attempt. Returns `None` after completion or
    /// when the configured attempt count is exhausted.
    pub fn next_text_attempt(&mut self, pending: &mut PendingText) -> Option<TextAttempt> {
        if pending.complete || pending.next_attempt >= pending.policy.attempts {
            return None;
        }
        let peer = self
            .contacts
            .iter()
            .find(|(key, _)| *key == pending.to)?
            .1
            .identity
            .clone();
        let secret = self.identity.shared_secret(&peer)?;
        let attempt = pending.next_attempt;
        let flood_last = pending.policy.flood_last
            && attempt + 1 == pending.policy.attempts
            && self.route_to(pending.to).is_some();
        if flood_last {
            self.clear_route(pending.to);
        }

        let mut message = TextMessage::plain(pending.timestamp, pending.text.clone());
        message.attempt = attempt;
        let ack = message.ack_crc(&self.me.pub_key);
        let payload = message.try_encode(&secret, pending.to, self.my_hash())?;
        let mut packet = Packet::new(ROUTE_FLOOD, payload_type::TXT_MSG);
        packet.payload = payload;
        let frame = self.route_outgoing(pending.to, packet);
        let flooded = Packet::decode(&frame).is_some_and(|packet| packet.is_flood());

        pending.next_attempt += 1;
        pending.expected_acks[pending.expected_ack_count as usize] = ack;
        pending.expected_ack_count += 1;
        Some(TextAttempt {
            frame,
            ack,
            attempt,
            flooded,
        })
    }

    fn route_outgoing(&mut self, to: u8, mut packet: Packet) -> Vec<u8> {
        if let Some(route) = self
            .contacts
            .iter()
            .find(|(key, _)| *key == to)
            .and_then(|(_, c)| c.route.clone())
        {
            packet.header = (packet.header & !0x03) | ROUTE_DIRECT;
            packet.path_len = route.path_len;
            packet.path = route.path;
        }
        self.seal_outgoing(&mut packet)
    }

    /// Record a packet we are about to transmit as seen, so its echo off the air is suppressed.
    pub(super) fn seal_outgoing(&mut self, packet: &mut Packet) -> Vec<u8> {
        if packet.is_flood() {
            // Newly originated flood frames have no recorded hops yet.
            packet.path_len = (self.flood_hash_size - 1) << 6;
        }
        self.seen.has_seen(packet);
        packet.encode()
    }

    /// A fallible flood advert frame for borrowed application data.
    pub fn try_advert_frame(
        &mut self,
        timestamp: u32,
        app_data: &[u8],
    ) -> Result<Vec<u8>, CapacityError> {
        let payload = Advert::encode(&self.identity, timestamp, app_data)
            .ok_or(CapacityError::AdvertDataTooLong)?;
        let mut packet = Packet::new(ROUTE_FLOOD, payload_type::ADVERT);
        packet.payload = payload;
        Ok(self.seal_outgoing(&mut packet))
    }

    /// A flood advert frame carrying our identity and `app_data`, to broadcast.
    pub fn advert_frame(&mut self, timestamp: u32, app_data: &[u8]) -> Vec<u8> {
        self.try_advert_frame(timestamp, app_data)
            .expect("advert app_data fits MeshCore frame")
    }

    /// A flood advert using the current structured MeshCore application data.
    pub fn advert_frame_data(&mut self, timestamp: u32, data: &AdvertData) -> Option<Vec<u8>> {
        Some(self.advert_frame(timestamp, &data.encode()?))
    }

    /// A flood text-message frame to a known contact `to`. `None` if `to` is unknown (we need
    /// its public key to derive the cipher key). Returns the frame and the ack to await.
    pub fn text_frame(&mut self, to: u8, timestamp: u32, text: &str) -> Option<(Vec<u8>, [u8; 4])> {
        let mut pending = self.begin_text(to, timestamp, text, TextRetryPolicy::default())?;
        let attempt = self.next_text_attempt(&mut pending)?;
        Some((attempt.frame, attempt.ack))
    }

    /// A flood ACK frame carrying `ack`.
    pub fn ack_frame(&mut self, ack: [u8; 4]) -> Vec<u8> {
        let mut packet = Packet::new(ROUTE_FLOOD, payload_type::ACK);
        packet.payload = encode_ack(ack);
        self.seal_outgoing(&mut packet)
    }

    /// An ACK to a known contact, sent directly when a route is known and flooded otherwise.
    pub fn ack_frame_to(&mut self, to: u8, ack: [u8; 4]) -> Vec<u8> {
        let mut packet = Packet::new(ROUTE_FLOOD, payload_type::ACK);
        packet.payload = encode_ack(ack);
        self.route_outgoing(to, packet)
    }
}
