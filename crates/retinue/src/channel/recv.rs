//! The receive side of [`Channel`]: in-order delivery, reordering, and backpressure.

use alloc::vec::Vec;

use super::{Channel, Envelope, SEQ_MODULUS};

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize>
    Channel<WINDOW, QUEUE, REORDER>
{
    /// Process a received envelope, delivering it or buffering it for reordering.
    ///
    /// Returns whether the driver should prove the underlying packet: yes for in-order,
    /// buffered, and duplicate frames. The proof is withheld only when the frame cannot be
    /// kept, because a *proved* frame that is dropped is lost forever, while an unproved one
    /// is retransmitted. That bounds the reorder buffer against a peer that sends only gaps.
    #[must_use]
    pub fn handle(&mut self, envelope: Envelope) -> bool {
        let ahead = envelope.sequence.wrapping_sub(self.recv_next);
        if ahead == 0 {
            // A full inbox means the application is not reading: withhold the proof
            // (backpressure).
            if self.inbox.is_full() {
                return false;
            }
            let _ = self.inbox.push_back((envelope.msgtype, envelope.payload));
            self.recv_next = self.recv_next.wrapping_add(1);
            self.pump();
            true
        } else if (ahead as u32) < SEQ_MODULUS / 2 {
            // A future sequence (forward half of the space): hold it if there is room.
            if self.reorder.is_full() && !self.reorder.contains_key(&envelope.sequence) {
                return false;
            }
            let _ = self
                .reorder
                .entry(envelope.sequence)
                .or_insert((envelope.msgtype, envelope.payload));
            true
        } else {
            // An already-delivered duplicate: our earlier proof was lost, so prove it again.
            true
        }
    }

    /// Move contiguous frames from the reorder buffer into the inbox while there is room.
    ///
    /// This must also run on the *read* path. A buffered frame was proved on arrival and will
    /// never be retransmitted; if the inbox fills mid-drain it waits in `reorder` at
    /// `recv_next`, and only the application making room can free it.
    fn pump(&mut self) {
        while !self.inbox.is_full() {
            let Some(next) = self.reorder.remove(&self.recv_next) else {
                break;
            };
            let _ = self.inbox.push_back(next);
            self.recv_next = self.recv_next.wrapping_add(1);
        }
    }

    /// The next in-order application payload, if one is ready, whatever its msgtype.
    /// Use [`recv_message`](Self::recv_message) to dispatch on the type.
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        self.recv_message().map(|(_, payload)| payload)
    }

    /// The next in-order message as `(msgtype, payload)`, if one is ready.
    ///
    /// Every message is sequenced and proved whatever its type, so an unexpected type
    /// never stalls the sequence; deciding what a type means is the reader's business.
    pub fn recv_message(&mut self) -> Option<(u16, Vec<u8>)> {
        // Pump first, so this never returns `None` while in-order data waits in `reorder`.
        self.pump();
        self.inbox.pop_front()
    }
}
