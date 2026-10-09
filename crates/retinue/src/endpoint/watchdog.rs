//! The link watchdog: keepalives, staleness, and handshake timeouts.

use alloc::vec::Vec;

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::link;
use crate::link_liveness::{self, Due};
use crate::packet::{Packet, PacketType};

use super::entropy::next_iv;
use super::queue::TrafficClass;
use super::shared::Shared;

/// How often the link watchdog advances every link's keepalive, staleness and handshake
/// timers (see [`crate::link_liveness`]). The shortest keepalive interval is 5 s.
pub(super) const LINK_WATCHDOG_TICK: Duration = Duration::from_millis(500);

impl Shared {
    /// The monotonic millisecond clock link liveness runs on.
    pub(super) fn link_clock_ms(&self) -> u64 {
        self.announce_admission_now_ms()
    }
}

/// Advance every link's liveness timers: send due keepalives, and drop links that went
/// stale (with a LINKCLOSE) or whose initiator never reported an RTT. A dropped link's
/// stream reads end in [`io::ErrorKind::TimedOut`], its session's in
/// [`io::ErrorKind::BrokenPipe`].
pub(super) fn watch_links(shared: &Shared) {
    let now = shared.link_clock_ms();
    let sends = shared.write_diagnostic(|| {
        let mut links = shared.links.lock().unwrap();
        let mut sends = Vec::new();
        let mut lost = Vec::new();
        for (id, entry) in links.iter_mut() {
            match entry.liveness.poll(now) {
                None => {}
                Some(Due::Keepalive) => sends.push((
                    entry.iface,
                    entry.link.keepalive_packet(link::KEEPALIVE_REQUEST),
                )),
                Some(Due::Teardown) => {
                    sends.push((entry.iface, entry.link.close_packet(&next_iv())));
                    lost.push(*id);
                }
                Some(Due::HandshakeTimeout) => lost.push(*id),
            }
        }
        for id in &lost {
            if let Some(entry) = links.remove(id) {
                entry.lost.store(true, Ordering::Release);
            }
        }
        (sends, !lost.is_empty())
    });
    for (iface, packet) in sends {
        shared.send_on_class(iface, packet, TrafficClass::Control);
    }
}

/// Feed an inbound packet on one of our links to its liveness timers, answering a keepalive
/// request when due. Returns whether the packet was liveness upkeep (a keepalive or the RTT
/// packet), which nothing else consumes.
pub(super) fn note_link_inbound(shared: &Shared, pkt: &Packet) -> bool {
    let now = shared.link_clock_ms();
    let reply = {
        let mut links = shared.links.lock().unwrap();
        let Some(entry) = links.get_mut(&pkt.destination) else {
            return false;
        };
        match pkt.context {
            link::CTX_KEEPALIVE if pkt.packet_type == PacketType::Data => {
                entry.liveness.on_keepalive(&pkt.payload, now).then(|| {
                    (
                        entry.iface,
                        entry.link.keepalive_packet(link::KEEPALIVE_RESPONSE),
                    )
                })
            }
            link::CTX_LRRTT if pkt.packet_type == PacketType::Data => {
                if let Some(rtt) = link_liveness::read_rtt(&entry.link, pkt) {
                    entry.liveness.on_rtt(rtt, now);
                }
                None
            }
            _ => {
                entry.liveness.on_inbound(now);
                return false;
            }
        }
    };
    if let Some((iface, packet)) = reply {
        shared.send_on_class(iface, packet, TrafficClass::Control);
    }
    true
}
