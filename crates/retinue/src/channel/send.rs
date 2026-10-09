//! The send side of [`Channel`]: windowed transmission, retransmission, and proof handling.

use alloc::vec::Vec;

use super::state::Outstanding;
use super::window::{
    FAST_RATE_THRESHOLD, RTT_FAST, RTT_MEDIUM, WINDOW_MAX, WINDOW_MAX_FAST, WINDOW_MAX_MEDIUM,
    WINDOW_MIN_LIMIT_FAST, WINDOW_MIN_LIMIT_MEDIUM, retx_timeout,
};
use super::{Channel, ChannelError, Envelope};

impl<const WINDOW: usize, const QUEUE: usize, const REORDER: usize>
    Channel<WINDOW, QUEUE, REORDER>
{
    /// The envelopes to transmit at time `now`: retransmissions of outstanding envelopes
    /// past their timeout, then newly sendable data within the window. There is no ack
    /// envelope — acknowledgement is the link proof, delivered via
    /// [`on_proof`](Self::on_proof).
    ///
    /// An envelope that times out on its last try fails the channel instead (see
    /// [`error`](Self::error)): this returns nothing then or after.
    pub fn poll_transmit(&mut self, now: u64) -> Vec<Envelope> {
        let mut out = Vec::new();
        if self.error.is_some() {
            return out;
        }

        // Retransmit anything unproved past its deadline, or give up on it (RNS
        // `_packet_timeout`). Each timeout steps the window down by one.
        let mut timeouts = 0u32;
        let mut exhausted = None;
        for (&seq, o) in &mut self.outstanding {
            if now < o.deadline {
                continue;
            }
            if o.tries >= self.max_tries {
                exhausted = Some(seq);
                break;
            }
            o.tries += 1;
            o.last_tx = now;
            o.deadline = 0; // recomputed below, once the in-flight count is settled
            out.push(Envelope {
                msgtype: self.msgtype,
                sequence: seq,
                payload: o.payload.clone(),
            });
            timeouts += 1;
        }
        if let Some(sequence) = exhausted {
            self.fail(ChannelError::RetriesExhausted { sequence });
            return Vec::new();
        }
        if self.dynamic {
            for _ in 0..timeouts {
                if self.window > self.window_min {
                    self.window -= 1;
                    if self.window_max > self.window_min + self.window_flexibility {
                        self.window_max -= 1;
                    }
                }
            }
        }

        // Fill the window with fresh data. The window is clamped to WINDOW at construction;
        // the `is_full` guard keeps that a local fact.
        //
        // The window also bounds the sequence *span*. An RNS receiver drops (yet the link
        // proves) any envelope more than WINDOW_MAX past its next expected sequence, so a new
        // sequence is assigned only within WINDOW_MAX of the oldest unproved one. That oldest
        // cannot change inside this loop: everything assigned here is newer.
        let oldest = self.oldest_outstanding();
        while (self.outstanding.len() as u32) < self.window
            && !self.outstanding.is_full()
            && u32::from(self.send_next.wrapping_sub(oldest)) < WINDOW_MAX
        {
            let Some(payload) = self.outgoing.pop_front() else {
                break;
            };
            let seq = self.send_next;
            self.send_next = self.send_next.wrapping_add(1);
            out.push(Envelope {
                msgtype: self.msgtype,
                sequence: seq,
                payload: payload.clone(),
            });
            let record = Outstanding {
                payload,
                last_tx: now,
                deadline: 0,
                tries: 1,
            };
            if let Err((_, unsent)) = self.outstanding.insert(seq, record) {
                // Unreachable while the guard above holds. Put the payload back and undo the
                // sequence rather than dropping application data on the floor.
                self.send_next = self.send_next.wrapping_sub(1);
                out.pop();
                let _ = self.outgoing.push_front(unsent.payload);
                break;
            }
        }

        // Anything sent sets its own deadline, and every in-flight envelope's deadline may
        // only move later now that more share the link (RNS `_update_packet_timeouts`).
        if !out.is_empty() {
            let in_flight = self.outstanding.len();
            let (dynamic, rtt, fixed) = (self.dynamic, self.rtt, self.retx_timeout);
            for o in self.outstanding.values_mut() {
                let timeout = if dynamic {
                    retx_timeout(rtt, o.tries, in_flight)
                } else {
                    fixed
                };
                o.deadline = o.deadline.max(o.last_tx.saturating_add(timeout));
            }
        }

        out
    }

    /// Give up: record why and drop everything unsent or unproved (RNS `_shutdown`).
    fn fail(&mut self, error: ChannelError) {
        self.error = Some(error);
        self.outstanding.clear();
        self.outgoing.clear();
    }

    /// The oldest unproved sequence, or `send_next` when nothing is in flight. Age is the
    /// wrapping distance back from `send_next`, so this holds across the 16-bit wrap.
    fn oldest_outstanding(&self) -> u16 {
        self.outstanding
            .keys()
            .copied()
            .max_by_key(|&seq| self.send_next.wrapping_sub(seq))
            .unwrap_or(self.send_next)
    }

    /// Release an outstanding sequence: its packet's proof arrived. RNS proves each packet
    /// individually, so this frees exactly one sequence. `now` lets the window measure RTT.
    ///
    /// Each proof opens the window by one up to `window_max`, and `FAST_RATE_THRESHOLD`
    /// proofs inside a faster RTT tier promote `window_max` and `window_min` to that tier
    /// (RNS `_packet_tx_op`). RTT is sampled only from envelopes sent once (Karn's rule): a
    /// proof of a retransmitted envelope cannot say which transmission it answers. The first
    /// sample replaces the initial estimate; later samples are averaged in.
    pub fn on_proof(&mut self, sequence: u16, now: u64) {
        let Some(o) = self.outstanding.remove(&sequence) else {
            return;
        };
        if !self.dynamic {
            return;
        }
        if o.tries == 1 {
            // Replace the guess outright: RNS starts from the measured handshake RTT.
            let sample = now.saturating_sub(o.last_tx);
            self.rtt = if self.rtt_measured {
                (self.rtt * 7 + sample) / 8
            } else {
                sample
            };
            self.rtt_measured = true;
        }
        if self.window < self.window_max {
            self.window += 1;
        }
        let ceiling = self.max_window;
        if self.rtt > RTT_FAST {
            self.fast_rate_rounds = 0;
            if self.rtt > RTT_MEDIUM {
                self.medium_rate_rounds = 0;
            } else {
                self.medium_rate_rounds = self.medium_rate_rounds.saturating_add(1);
                if self.window_max < WINDOW_MAX_MEDIUM
                    && self.medium_rate_rounds == FAST_RATE_THRESHOLD
                {
                    self.window_max = WINDOW_MAX_MEDIUM.min(ceiling);
                    self.window_min = WINDOW_MIN_LIMIT_MEDIUM.min(ceiling);
                }
            }
        } else {
            self.fast_rate_rounds = self.fast_rate_rounds.saturating_add(1);
            if self.window_max < WINDOW_MAX_FAST && self.fast_rate_rounds == FAST_RATE_THRESHOLD {
                self.window_max = WINDOW_MAX_FAST.min(ceiling);
                self.window_min = WINDOW_MIN_LIMIT_FAST.min(ceiling);
            }
        }
    }
}
