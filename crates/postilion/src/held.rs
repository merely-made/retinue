//! Messages from senders not yet heard announcing, held until they are.
//!
//! Direct delivery proves a message on arrival, as stock LXMF does, so its sender will not
//! send it again. Refusing one whose sender we cannot yet verify would lose it; holding it
//! until the sender's announce brings its keys does not.

use std::collections::VecDeque;
use std::sync::Mutex;

use outrider::{DeliveredCache, ReceivedDirect, Verification, reverify};
use retinue::endpoint::Endpoint;
use retinue::hash::AddressHash;

use crate::Event;

/// Messages held at once. Past it the oldest is dropped, and said so.
const HELD_MESSAGES: usize = 16;

#[derive(Default)]
pub(crate) struct Held {
    pub(crate) delivered: DeliveredCache,
    waiting: Mutex<VecDeque<ReceivedDirect>>,
}

impl Held {
    /// Hold an unverified message, returning the event for any it pushed out. It is settled
    /// at once if its sender's announce arrived while it was being received.
    pub(crate) fn hold(
        &self,
        endpoint: &Endpoint,
        received: ReceivedDirect,
        now: f64,
    ) -> Vec<Event> {
        let from = source(&received);
        let dropped = {
            let mut waiting = self.waiting.lock().unwrap();
            waiting.push_back(received);
            (waiting.len() > HELD_MESSAGES)
                .then(|| waiting.pop_front())
                .flatten()
        };
        let mut events: Vec<Event> = dropped
            .map(|dropped| {
                Event::Dropped(format!(
                    "unverified message from {}: its sender never announced",
                    source(&dropped)
                ))
            })
            .into_iter()
            .collect();
        events.extend(self.release(endpoint, from, now));
        events
    }

    /// Settle the messages `from` sent, now that it has announced.
    pub(crate) fn release(&self, endpoint: &Endpoint, from: AddressHash, now: f64) -> Vec<Event> {
        let ready = {
            let mut waiting = self.waiting.lock().unwrap();
            let (ready, rest): (VecDeque<_>, _) =
                waiting.drain(..).partition(|held| source(held) == from);
            *waiting = rest;
            ready
        };
        let mut events = Vec::new();
        for mut received in ready {
            let (verification, identity) = reverify(endpoint, &received.message);
            match verification {
                Verification::Verified => {
                    if self.delivered.admit(received.message.message_id, now) {
                        received.verification = verification;
                        received.source_identity = identity;
                        events.push(Event::authenticated_message(received));
                    }
                }
                Verification::SourceUnknown => {
                    self.waiting.lock().unwrap().push_back(received);
                }
                Verification::SignatureInvalid => events.push(Event::Dropped(format!(
                    "message from {from} does not verify against its announce"
                ))),
            }
        }
        events
    }
}

fn source(received: &ReceivedDirect) -> AddressHash {
    AddressHash::from_bytes(received.message.source)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use outrider::{
        DeliveryAnnounce, LxmfPayload, announce_delivery, delivery_destination,
        receive_direct_with_stamp_cost, register_delivery, send_direct,
    };
    use retinue::identity::PrivateIdentity;
    use retinue::packet::PacketType;

    use super::*;

    /// Connect the endpoints, carrying `quiet`'s announces only once `heard` is set.
    fn connect_muting(a: &Endpoint, b: &Endpoint, quiet: AddressHash, heard: Arc<AtomicBool>) {
        let (mut a_out, a_sink) = a.attach_interface().split();
        let (mut b_out, b_sink) = b.attach_interface().split();
        tokio::spawn(async move {
            while let Some(packet) = a_out.recv().await {
                let muted = packet.packet_type == PacketType::Announce
                    && packet.destination == quiet
                    && !heard.load(Ordering::SeqCst);
                if !muted && !b_sink.deliver(packet) {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            while let Some(packet) = b_out.recv().await {
                if !a_sink.deliver(packet) {
                    break;
                }
            }
        });
    }

    #[tokio::test]
    async fn a_held_message_is_released_once_its_sender_announces() {
        let sender_identity = PrivateIdentity::from_secret_bytes(&[0x53; 64]);
        let sender = Arc::new(Endpoint::new(sender_identity.clone()));
        let receiver = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
            &[0x63; 64],
        )));
        let from = delivery_destination(sender_identity.public());
        let heard = Arc::new(AtomicBool::new(false));
        connect_muting(&sender, &receiver, from, Arc::clone(&heard));
        let announce = DeliveryAnnounce::named(b"Quiet".to_vec());
        register_delivery(&sender, &announce).unwrap();
        register_delivery(&receiver, &DeliveryAnnounce::named(b"Receiver".to_vec())).unwrap();
        let peer = tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
            .await
            .unwrap()
            .unwrap();

        let held = Held::default();
        let receiving = tokio::spawn({
            let receiver = Arc::clone(&receiver);
            async move {
                let accepted = receiver.accept_resource().await.unwrap();
                receive_direct_with_stamp_cost(
                    &receiver,
                    accepted,
                    &DeliveredCache::default(),
                    4096,
                    None,
                )
                .await
                .unwrap()
            }
        });
        let payload = LxmfPayload::text(1_753_603_205.5, b"", b"held".to_vec());
        send_direct(&sender, &sender_identity, &peer, &payload)
            .await
            .unwrap();
        let received = receiving.await.unwrap();
        assert_eq!(received.verification, Verification::SourceUnknown);
        assert!(held.hold(&receiver, received.clone(), 1.0).is_empty());

        heard.store(true, Ordering::SeqCst);
        announce_delivery(&sender, &announce).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while receiver.next_announcement().await.unwrap().destination != from {}
        })
        .await
        .unwrap();
        let released = held.release(&receiver, from, 2.0);
        assert!(matches!(
            released.as_slice(),
            [Event::Message { message_id, .. }] if *message_id == received.message.message_id
        ));

        // A second copy settles at once, as a duplicate: no event.
        assert!(held.hold(&receiver, received.clone(), 3.0).is_empty());
        // Held after the announce was handled, a new message is settled at once.
        let fresh = Held::default();
        assert!(matches!(
            fresh.hold(&receiver, received, 4.0).as_slice(),
            [Event::Message { .. }]
        ));
    }
}
