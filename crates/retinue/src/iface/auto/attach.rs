//! Starting an AutoInterface: adopt interfaces, open their sockets, spawn their carriers.

use alloc::vec::Vec;

use std::io;
use std::net::Ipv6Addr;
use std::sync::Arc;

use tokio::sync::watch;

use super::handle::AutoHandle;
use super::run::Carrier;
use super::sockets::Sockets;
use super::state::State;
use super::{Adopted, AutoConfig, select};
use crate::auto::discovery_group;
use crate::endpoint::Endpoint;
use crate::iface::netinfo;

/// Where a carrier re-reads its interface's link-locals on each job.
pub(super) type LinkLocals = fn(&str) -> Option<Vec<Ipv6Addr>>;

fn host_link_locals(name: &str) -> Option<Vec<Ipv6Addr>> {
    Some(netinfo::find(name).ok()??.link_local)
}

impl Endpoint {
    /// Attach an RNS AutoInterface on every host interface [`select`] adopts.
    ///
    /// Fails if none can be adopted, if the endpoint is closed, or if the data port is
    /// taken on one: another instance on this host shares its link-locals, so the two could
    /// never peer. Closing the endpoint stops discovery as dropping the handle does.
    pub async fn attach_auto(self: &Arc<Self>, cfg: AutoConfig) -> io::Result<AutoHandle> {
        let adopted = select(&netinfo::interfaces()?, &cfg);
        attach_adopted(self, cfg, adopted, Some(host_link_locals))
    }
}

/// Start discovery on `adopted`. With `follow`, each job re-reads the interface's
/// link-locals from it.
pub(super) fn attach_adopted(
    ep: &Arc<Endpoint>,
    cfg: AutoConfig,
    adopted: Vec<Adopted>,
    follow: Option<LinkLocals>,
) -> io::Result<AutoHandle> {
    let group = discovery_group(&cfg.group_id, cfg.scope, cfg.addr_type);
    let mut opened = Vec::new();
    for a in adopted {
        match Sockets::open(
            group,
            a.link_local,
            a.index,
            cfg.discovery_port,
            cfg.data_port,
        ) {
            Ok(sockets) => opened.push((a, sockets)),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => return Err(e),
            // Stock skips an interface it cannot configure (`AutoInterface.py` 314-319).
            Err(_) => {}
        }
    }
    if opened.is_empty() {
        let why = "no interface with an IPv6 link-local address could be adopted";
        return Err(io::Error::new(io::ErrorKind::NotFound, why));
    }
    let names = opened.iter().map(|(a, _)| (a.index, a.name.clone()));
    let state = Arc::new(State::new(names.collect()));
    let (stop, stopped) = watch::channel(false);
    let handle = AutoHandle::new(Arc::clone(&state), stop);
    let cfg = Arc::new(cfg);
    for (a, sockets) in opened {
        state
            .table
            .lock()
            .unwrap()
            .adopt(a.index, a.link_local, &a.all, state.now());
        let carrier = Carrier {
            ep: Arc::downgrade(ep),
            state: Arc::clone(&state),
            cfg: Arc::clone(&cfg),
            group,
            name: a.name,
            index: a.index,
            link_local: a.link_local,
            data: watch::channel(Arc::clone(&sockets.data)).0,
            sockets,
            follow,
        };
        // Tracked, so closing the endpoint stops the carrier and frees its ports.
        ep.spawn_carrier(carrier.run(stopped.clone()))?;
    }
    Ok(handle)
}
