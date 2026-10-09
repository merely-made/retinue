//! What a running AutoInterface reports, and the handle that stops it.

use alloc::string::String;
use alloc::vec::Vec;

use std::net::Ipv6Addr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::watch;

use super::run::State;
use crate::endpoint::InterfaceId;

/// An adopted interface as [`AutoHandle::status`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdoptedStatus {
    pub name: String,
    pub index: u32,
    pub link_local: Ipv6Addr,
    /// False while none of our own tokens has come back for 6.5 s: multicast is not
    /// reaching the link. Stock only logs this (`AutoInterface.py` 462-480).
    pub carrier_up: bool,
    /// Whether any own token has come back since adoption.
    pub echoed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStatus {
    pub addr: Ipv6Addr,
    pub index: u32,
    pub interface: InterfaceId,
    pub silent: Duration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoCounters {
    pub rx: u64,
    pub tx: u64,
    /// Datagrams dropped as the same bytes from another peer within 0.75 s.
    pub mif_duplicates: u64,
    pub oversize: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoStatus {
    pub adopted: Vec<AdoptedStatus>,
    pub peers: Vec<PeerStatus>,
    pub counters: AutoCounters,
}

/// A running AutoInterface. Dropping it, or [`Self::close`], stops discovery and detaches
/// every peer interface.
pub struct AutoHandle {
    state: Arc<State>,
    stop: watch::Sender<bool>,
}

impl AutoHandle {
    pub(super) fn new(state: Arc<State>, stop: watch::Sender<bool>) -> Self {
        Self { state, stop }
    }

    pub fn status(&self) -> AutoStatus {
        let (state, now) = (&self.state, self.state.now());
        let table = state.table.lock().unwrap();
        let name = |index| {
            state
                .names
                .iter()
                .find(|(i, _)| *i == index)
                .map(|(_, n)| n)
        };
        let c = &state.counters;
        AutoStatus {
            adopted: table
                .carriers()
                .iter()
                .map(|c| AdoptedStatus {
                    name: name(c.index).cloned().unwrap_or_default(),
                    index: c.index,
                    link_local: c.link_local,
                    carrier_up: c.up,
                    echoed: c.echoed,
                })
                .collect(),
            peers: table
                .peers()
                .iter()
                .map(|p| PeerStatus {
                    addr: p.addr,
                    index: p.index,
                    interface: p.value.id,
                    silent: Duration::from_millis(now.saturating_sub(p.last_heard)),
                })
                .collect(),
            counters: AutoCounters {
                rx: c.rx.load(Ordering::Relaxed),
                tx: c.tx.load(Ordering::Relaxed),
                mif_duplicates: c.mif_duplicates.load(Ordering::Relaxed),
                oversize: c.oversize.load(Ordering::Relaxed),
            },
        }
    }

    pub fn close(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for AutoHandle {
    fn drop(&mut self) {
        self.close();
    }
}
