//! What the carriers of one AutoInterface share, and the per-peer send pump.

use alloc::string::String;
use alloc::vec::Vec;

use std::net::SocketAddrV6;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::net::UdpSocket;
use tokio::sync::watch;

use super::BITRATE_GUESS;
use super::peers::Table;
use crate::endpoint::{Endpoint, IfacePolicy, InterfaceId, InterfaceSink, OutboundPackets};

pub(super) struct Peer {
    pub id: InterfaceId,
    pub sink: InterfaceSink,
}

#[derive(Default)]
pub(super) struct Counters {
    pub rx: AtomicU64,
    pub tx: AtomicU64,
    pub mif_duplicates: AtomicU64,
    pub oversize: AtomicU64,
    pub peers_refused: AtomicU64,
}

pub(super) struct State {
    epoch: Instant,
    pub table: Mutex<Table<Peer>>,
    pub names: Vec<(u32, String)>,
    pub counters: Counters,
}

impl State {
    pub fn new(names: Vec<(u32, String)>) -> Self {
        Self {
            epoch: Instant::now(),
            table: Mutex::new(Table::default()),
            names,
            counters: Counters::default(),
        }
    }

    pub fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Give a peer interface its parent's policy, as stock copies it (`AutoInterface.py`
/// 543-593). An unset bitrate becomes [`BITRATE_GUESS`].
pub(super) fn apply_policy(ep: &Endpoint, id: InterfaceId, p: &IfacePolicy) {
    ep.set_interface_ingress_policy(id, p.ingress);
    ep.set_interface_announce_rate(id, p.announce_rate);
    ep.set_announce_cap(id, p.cap_percent);
    ep.set_interface_bitrate(id, Some(p.bitrate_bps.unwrap_or(BITRATE_GUESS)));
    ep.set_interface_gravity(id, p.gravity);
    ep.set_interface_outgoing(id, p.outgoing);
    ep.set_interface_mode_flags(id, p.announces_from_internal, p.announces_to_internal);
}

/// Send a peer's packets from the interface's current data socket until the endpoint
/// detaches it.
pub(super) async fn pump(
    mut out: OutboundPackets,
    data: watch::Receiver<Arc<UdpSocket>>,
    to: SocketAddrV6,
    state: Arc<State>,
) {
    while let Some(packet) = out.recv().await {
        let Ok(wire) = out.encode(&packet) else {
            continue;
        };
        let socket = Arc::clone(&data.borrow());
        if socket.send_to(&wire, to).await.is_ok() {
            State::count(&state.counters.tx);
        }
    }
}
