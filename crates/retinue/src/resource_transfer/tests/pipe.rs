//! A virtual-clock link for driving both halves of a transfer: bandwidth, latency and loss.

use alloc::vec::Vec;

use super::*;
use crate::link::{CTX_RESOURCE, CTX_RESOURCE_REQ};
use crate::lossy::LossModel;
use crate::resource::parse_request;

/// One direction of a link: packets serialise at `rate` bytes per second, arrive `latency`
/// ms after their last byte leaves, and the loss model drops some.
pub(super) struct Pipe {
    rate: u64,
    latency: u64,
    free_at: u64,
    loss: Option<LossModel>,
    in_flight: Vec<(u64, Packet)>,
}

impl Pipe {
    pub(super) fn new(rate: u64, latency: u64) -> Self {
        Self {
            rate,
            latency,
            free_at: 0,
            loss: None,
            in_flight: Vec::new(),
        }
    }

    pub(super) fn lossy(mut self, seed: u64, per_mille: u32) -> Self {
        self.loss = Some(LossModel::new(seed).drop_per_mille(per_mille));
        self
    }

    fn send(&mut self, now: u64, packet: Packet) {
        let sent = now.max(self.free_at) + packet.encoded_len() as u64 * 1_000 / self.rate;
        self.free_at = sent;
        if self.loss.as_mut().is_some_and(LossModel::should_drop) {
            return;
        }
        self.in_flight.push((sent + self.latency, packet));
    }

    fn next_arrival(&self) -> Option<u64> {
        self.in_flight.first().map(|(at, _)| *at)
    }

    fn take_due(&mut self, now: u64) -> Vec<Packet> {
        let due = self.in_flight.partition_point(|(at, _)| *at <= now);
        self.in_flight
            .drain(..due)
            .map(|(_, packet)| packet)
            .collect()
    }
}

/// What one run saw.
#[derive(Debug, Default)]
pub(super) struct Run {
    /// Requests the receiver sent, and those that asked for no part.
    pub(super) requests: usize,
    pub(super) empty_requests: usize,
    /// Part packets that reached the receiver.
    pub(super) parts_heard: usize,
    pub(super) max_window: usize,
    pub(super) elapsed: u64,
}

/// Drive `sender` to `receiver` over `fwd` (and requests back over `bwd`) until the sender
/// concludes, or `limit` ms pass.
pub(super) fn run(
    sender: &mut ResourceSender,
    receiver: &mut ResourceReceiver,
    mut fwd: Pipe,
    mut bwd: Pipe,
    limit: u64,
) -> Run {
    let mut ivg = iv_gen();
    let mut stats = Run::default();
    let mut now = 0;
    fwd.send(now, sender.advertise(now, &ivg()));
    while !sender.is_done() && !sender.is_canceled() {
        let next = [
            fwd.next_arrival(),
            bwd.next_arrival(),
            sender.deadline(),
            receiver.deadline(),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(next) = next.filter(|&next| next <= limit) else {
            break;
        };
        now = now.max(next);
        let mut back = Vec::new();
        for packet in fwd.take_due(now) {
            stats.parts_heard += usize::from(packet.context == CTX_RESOURCE);
            back.extend(receiver.on_packet(&packet, now, &mut ivg));
        }
        back.extend(receiver.poll(now, &mut ivg));
        for packet in back {
            if packet.context == CTX_RESOURCE_REQ {
                let request = parse_request(&sender.link.decrypt(&packet).unwrap()).unwrap();
                stats.requests += 1;
                stats.empty_requests += usize::from(request.wanted.is_empty());
            }
            bwd.send(now, packet);
        }
        stats.max_window = stats.max_window.max(receiver.window());
        let mut forth: Vec<Packet> = bwd
            .take_due(now)
            .iter()
            .flat_map(|packet| sender.on_packet(packet, now, &mut ivg))
            .collect();
        forth.extend(sender.poll(now, &mut ivg));
        for packet in forth {
            fwd.send(now, packet);
        }
    }
    stats.elapsed = now;
    stats
}

/// A sender and receiver over one link, timed for a link of `rtt` ms.
pub(super) fn pair(data: &[u8], rtt: u64) -> (ResourceSender, ResourceReceiver) {
    let (send_link, recv_link) = link_pair();
    let timing = Timing { rtt, floor: 0 };
    let sender =
        ResourceSender::publish(send_link, data, [7, 7, 7, 7], &[1; IV_LEN]).with_timing(timing);
    (sender, ResourceReceiver::new(recv_link).with_timing(timing))
}
