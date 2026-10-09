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
    /// Hold an unverified message, returning the event for any it pushed out.
    pub(crate) fn hold(&self, received: ReceivedDirect) -> Option<Event> {
        let mut waiting = self.waiting.lock().unwrap();
        waiting.push_back(received);
        (waiting.len() > HELD_MESSAGES)
            .then(|| waiting.pop_front())
            .flatten()
            .map(|dropped| {
                Event::Dropped(format!(
                    "unverified message from {}: its sender never announced",
                    source(&dropped)
                ))
            })
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
