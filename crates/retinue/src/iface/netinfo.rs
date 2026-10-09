//! Host interfaces and their addresses, as RNS's `netinfo` lists them for AutoInterface and
//! the UDP `device` option (`netinfo.py` 57-79, 191-232).

use alloc::string::String;
use alloc::vec::Vec;

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};

use nix::ifaddrs::getifaddrs;
use nix::net::if_::{InterfaceFlags, if_nametoindex};

use crate::auto::descope;

/// One host interface, with every address `getifaddrs` reported for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetIf {
    pub name: String,
    /// From `if_nametoindex`: on macOS the scope in a `getifaddrs` link-local is not reliable
    /// (`AutoInterface.py` 95-101).
    pub index: u32,
    pub up: bool,
    pub multicast: bool,
    /// IPv4 addresses with their prefix length, in report order.
    pub v4: Vec<(Ipv4Addr, u8)>,
    /// `fe80::` addresses with any KAME scope cleared ([`descope`]), in report order.
    pub link_local: Vec<Ipv6Addr>,
}

impl NetIf {
    /// The subnet broadcast of the first IPv4 address, computed from its prefix as stock
    /// does rather than read from `ifa_broadaddr` (`netinfo.py` 64-71; `UDPInterface.py` 50-54).
    pub fn broadcast_v4(&self) -> Option<Ipv4Addr> {
        let &(addr, prefix) = self.v4.first()?;
        let host = u32::MAX.checked_shr(u32::from(prefix)).unwrap_or(0);
        Some(Ipv4Addr::from(addr.to_bits() | host))
    }
}

/// Every interface with at least one entry, in first-report order (`netinfo.py` 198-232).
/// An interface whose index cannot be resolved is left out: nothing can bind to it.
pub fn interfaces() -> io::Result<Vec<NetIf>> {
    let mut out: Vec<NetIf> = Vec::new();
    for entry in getifaddrs().map_err(io::Error::from)? {
        let at = match out.iter().position(|i| i.name == entry.interface_name) {
            Some(at) => at,
            None => {
                let Ok(index) = if_nametoindex(entry.interface_name.as_str()) else {
                    continue;
                };
                out.push(NetIf {
                    name: entry.interface_name.clone(),
                    index,
                    ..NetIf::default()
                });
                out.len() - 1
            }
        };
        let netif = &mut out[at];
        netif.up |= entry.flags.contains(InterfaceFlags::IFF_UP);
        netif.multicast |= entry.flags.contains(InterfaceFlags::IFF_MULTICAST);
        let Some(addr) = entry.address else { continue };
        if let Some(v4) = addr.as_sockaddr_in() {
            let mask = entry.netmask.as_ref().and_then(|m| m.as_sockaddr_in());
            let prefix = mask.map_or(32, |m| m.ip().to_bits().count_ones() as u8);
            netif.v4.push((v4.ip(), prefix));
        } else if let Some(v6) = addr.as_sockaddr_in6()
            && v6.ip().segments()[0] == 0xfe80
        {
            netif.link_local.push(descope(v6.ip()));
        }
    }
    Ok(out)
}

/// The interface called `name`, if the host has one.
pub fn find(name: &str) -> io::Result<Option<NetIf>> {
    Ok(interfaces()?.into_iter().find(|i| i.name == name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_from_prefix() {
        let netif = NetIf {
            v4: alloc::vec![(Ipv4Addr::new(192, 168, 4, 68), 22)],
            ..NetIf::default()
        };
        assert_eq!(netif.broadcast_v4(), Some(Ipv4Addr::new(192, 168, 7, 255)));
        let host = NetIf {
            v4: alloc::vec![(Ipv4Addr::new(10, 0, 0, 1), 32)],
            ..NetIf::default()
        };
        assert_eq!(host.broadcast_v4(), Some(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(NetIf::default().broadcast_v4(), None);
    }

    #[test]
    fn host_has_loopback() {
        let all = interfaces().unwrap();
        let lo = all
            .iter()
            .find(|i| i.v4.iter().any(|(a, _)| a.is_loopback()));
        let lo = lo.expect("a loopback interface");
        assert!(lo.up && lo.index > 0);
        assert_eq!(lo.v4.iter().find(|(a, _)| a.is_loopback()).unwrap().1, 8);
    }
}
