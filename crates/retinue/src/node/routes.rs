//! Learned routes and per-interface settings.

#[cfg(doc)]
use super::first_hop_airtime;
use super::tables::Route;
use super::{
    AirtimeTableFull, InterfaceId, InterfaceMode, InterfaceModeTableFull, NextHop, Node,
    REVERSE_TIMEOUT,
};
use crate::hash::AddressHash;

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// Number of fresh or not-yet-polled route entries currently held.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// A fresh route's radio interface and hop count. Lookup also evicts an expired entry, so
    /// a stale path does not linger until an unrelated new announce arrives.
    pub fn route_to(&mut self, destination: AddressHash, now: u64) -> Option<(InterfaceId, u8)> {
        self.expire_routes(now);
        self.routes
            .iter()
            .find(|route| route.destination == destination)
            .map(|route| (route.interface, route.hops))
    }

    /// A route's next hop, read-only. A route past its TTL is reported as absent but not
    /// evicted; [`Self::route_to`] and [`Self::poll`] do that.
    pub fn next_hop(&self, destination: AddressHash, now: u64) -> Option<NextHop> {
        self.routes
            .iter()
            .find(|route| {
                route.destination == destination && route.live(now, self.transport.route_ttl)
            })
            .map(|route| NextHop {
                interface: route.interface,
                via: route.transport,
                hops: route.hops,
            })
    }

    /// The first-hop airtime allowance a link request leaving by `interface` adds to its
    /// deadline, in milliseconds. Zero unless [`Self::set_first_hop_airtime`] set one.
    pub fn first_hop_airtime(&self, interface: InterfaceId) -> u64 {
        self.first_hop_airtime
            .iter()
            .find(|(id, _)| *id == interface)
            .map_or(0, |(_, allowance)| *allowance)
    }

    /// Set the first-hop airtime allowance for requests leaving by `interface`, in
    /// milliseconds; [`first_hop_airtime`] computes one from a bitrate. A radio shell sets
    /// it from its modulation, and an unbounded link such as TCP leaves it at zero. Zero
    /// clears the entry. Requests already pending keep the deadline they were given.
    pub fn set_first_hop_airtime(
        &mut self,
        interface: InterfaceId,
        allowance: u64,
    ) -> Result<(), AirtimeTableFull> {
        let existing = self
            .first_hop_airtime
            .iter()
            .position(|(id, _)| *id == interface);
        match (existing, allowance) {
            (Some(index), 0) => {
                self.first_hop_airtime.swap_remove(index);
            }
            (Some(index), _) => self.first_hop_airtime[index].1 = allowance,
            (None, 0) => {}
            (None, _) => self
                .first_hop_airtime
                .push((interface, allowance))
                .map_err(|_| AirtimeTableFull)?,
        }
        Ok(())
    }

    /// The mode of `interface`: [`InterfaceMode::Full`] unless [`Self::set_interface_mode`]
    /// set another.
    pub fn interface_mode(&self, interface: InterfaceId) -> InterfaceMode {
        self.interface_modes
            .iter()
            .find(|(id, _)| *id == interface)
            .map_or(InterfaceMode::Full, |(_, mode)| *mode)
    }

    /// Set the mode of `interface`, which bounds the lifetime of routes learned on it from
    /// now on (see [`InterfaceMode::route_ttl`]). [`InterfaceMode::Full`] clears the entry.
    pub fn set_interface_mode(
        &mut self,
        interface: InterfaceId,
        mode: InterfaceMode,
    ) -> Result<(), InterfaceModeTableFull> {
        let existing = self
            .interface_modes
            .iter()
            .position(|(id, _)| *id == interface);
        match (existing, mode) {
            (Some(index), InterfaceMode::Full) => {
                self.interface_modes.swap_remove(index);
            }
            (Some(index), _) => self.interface_modes[index].1 = mode,
            (None, InterfaceMode::Full) => {}
            (None, _) => self
                .interface_modes
                .push((interface, mode))
                .map_err(|_| InterfaceModeTableFull)?,
        }
        Ok(())
    }

    /// Forget a detached interface: the routes learned on it, the carried links that cross
    /// it, and its airtime and mode settings. Traffic for those destinations then goes out
    /// without a route, as for any unknown destination, instead of naming an interface that
    /// is gone. RNS culls the same rows when their interface disappears (`Transport.py`
    /// 880-881, 975-978).
    pub fn forget_interface(&mut self, interface: InterfaceId) {
        self.routes.retain(|route| route.interface != interface);
        self.bridges
            .retain(|bridge| bridge.from != interface && bridge.out != interface);
        self.first_hop_airtime.retain(|(id, _)| *id != interface);
        self.interface_modes.retain(|(id, _)| *id != interface);
    }

    /// Remove routes and carried-link records that have outlived the policy that admitted
    /// them. This is called both from [`Node::poll`] and before a transit decision, so a slow
    /// board clock cannot leave a stale route usable merely because it has not polled yet.
    pub(super) fn expire_transport_state(&mut self, now: u64) {
        self.expire_routes(now);
        while let Some(index) = self
            .bridges
            .iter()
            .position(|bridge| bridge.lapsed(now, self.transport.bridge_ttl))
        {
            self.bridges.swap_remove(index);
            self.transport_counters.expired_bridges =
                self.transport_counters.expired_bridges.saturating_add(1);
        }
        self.reverse
            .retain(|entry| now.saturating_sub(entry.seen) < REVERSE_TIMEOUT);
    }

    fn expire_routes(&mut self, now: u64) {
        while let Some(index) = self
            .routes
            .iter()
            .position(|route| !route.live(now, self.transport.route_ttl))
        {
            self.routes.swap_remove(index);
            self.transport_counters.expired_routes =
                self.transport_counters.expired_routes.saturating_add(1);
        }
    }

    /// Record a route from a freshness-accepted announce. The accepted announce is the route
    /// incumbent regardless of hop count. Freshness decides whether an announce may mutate any
    /// observable state; route selection must not apply a second shortest-path filter.
    pub(super) fn learn_route(
        &mut self,
        destination: AddressHash,
        interface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
        now: u64,
    ) {
        if destination == self.destination() {
            return;
        }
        self.expire_routes(now);
        let mode = self.interface_mode(interface);
        if let Some(route) = self
            .routes
            .iter_mut()
            .find(|route| route.destination == destination)
        {
            *route = Route {
                destination,
                interface,
                transport,
                hops,
                learned: now,
                mode,
            };
            return;
        }

        if self.routes.is_full()
            && let Some(index) = self
                .routes
                .iter()
                .enumerate()
                .min_by_key(|(_, route)| route.learned)
                .map(|(index, _)| index)
        {
            self.routes.swap_remove(index);
            self.transport_counters.evicted_routes =
                self.transport_counters.evicted_routes.saturating_add(1);
        }
        let _ = self.routes.push(Route {
            destination,
            interface,
            transport,
            hops,
            learned: now,
            mode,
        });
    }

    /// Mark a route used at `now`, which extends its life as RNS's path timestamp does.
    pub(super) fn touch_route(&mut self, destination: AddressHash, now: u64) {
        if let Some(route) = self
            .routes
            .iter_mut()
            .find(|route| route.destination == destination)
        {
            route.learned = now;
        }
    }
}
