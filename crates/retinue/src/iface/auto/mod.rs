//! RNS AutoInterface: zero-configuration peering over IPv6 link-local multicast
//! (`AutoInterface.py` 43-710).
//!
//! Every adopted host interface multicasts a peering token to the discovery group every
//! 1.6 s. A valid token from another address makes that address a peer, and each peer is
//! its own endpoint interface carrying one packet per unicast datagram. A peer silent for
//! 22 s is detached. Peers also get a unicast token every 5.2 s, so one-way multicast
//! (common on Wi-Fi) still peers both ways.

use alloc::string::String;
use alloc::vec::Vec;

use std::net::Ipv6Addr;
use std::time::Duration;

use super::netinfo::NetIf;
use crate::endpoint::IfacePolicy;
use crate::ifac::Ifac;
use crate::node::InterfaceMode;

pub use crate::auto::{AddrType, DATA_PORT, DEFAULT_GROUP, DISCOVERY_PORT, Scope};
pub use run::{AdoptedStatus, AutoCounters, AutoHandle, AutoStatus, PeerStatus};

mod peers;
mod run;
pub mod sockets;
#[cfg(test)]
mod tests;

/// Stock's fixed hardware MTU (`AutoInterface.py` 44-45). It bounds received frames; sent
/// packets keep the protocol MTU.
pub const HW_MTU: usize = 1196;
/// Stock's bitrate guess (`AutoInterface.py` 70, 326-329).
pub const BITRATE_GUESS: u64 = 10_000_000;
/// Token multicast period (`AutoInterface.py` 62).
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_millis(1600);
/// Peer expiry, reverse peering and carrier checks run this often (`AutoInterface.py` 63).
pub const PEER_JOB_INTERVAL: Duration = Duration::from_secs(4);
/// A peer silent this long is removed (`AutoInterface.py` 61; ×1.25 on Android, 149-152).
pub const PEERING_TIMEOUT: Duration = if cfg!(target_os = "android") {
    Duration::from_millis(27_500)
} else {
    Duration::from_secs(22)
};
/// Minimum gap between unicast tokens to one peer: 1.6 s × 3.25 (`AutoInterface.py` 147).
pub const REVERSE_INTERVAL: Duration = Duration::from_millis(5200);
/// No own token back for this long marks the carrier down (`AutoInterface.py` 64).
pub const MCAST_ECHO_TIMEOUT: Duration = Duration::from_millis(6500);
/// Multi-interface dedup depth and lifetime (`AutoInterface.py` 72-73).
pub const MIF_LEN: usize = 48;
pub const MIF_TTL: Duration = Duration::from_millis(750);

/// Skipped unless named in `devices` (`AutoInterface.py` 66-68); `lo0` always is.
const DARWIN_IGNORE: [&str; 4] = ["awdl0", "llw0", "lo0", "en5"];
const ANDROID_IGNORE: [&str; 11] = [
    "dummy0", "lo", "tun0", "rmnet0", "rmnet1", "rmnet2", "rmnet3", "rmnet4", "rmnet5", "rmnet6",
    "rmnet7",
];

/// An AutoInterface's configuration, with stock's defaults (`AutoInterface.py` 103-213).
#[derive(Clone, Debug)]
pub struct AutoConfig {
    pub group_id: Vec<u8>,
    pub scope: Scope,
    pub addr_type: AddrType,
    /// Multicast discovery port; reverse peering uses the next one.
    pub discovery_port: u16,
    pub data_port: u16,
    /// If not empty, only these interfaces are adopted.
    pub devices: Vec<String>,
    pub ignored_devices: Vec<String>,
    /// Stock peers without `ifac_size` use [`Ifac::for_stream`].
    pub ifac: Option<Ifac>,
    /// Mode and policy every peer interface gets, as stock copies its parent's settings
    /// (`AutoInterface.py` 543-593). An unset bitrate becomes [`BITRATE_GUESS`].
    pub mode: InterfaceMode,
    pub policy: IfacePolicy,
    /// Also skip interfaces that are not UP and MULTICAST; stock checks neither.
    pub strict: bool,
    /// Multicast tokens. Off, this node peers only when it hears another's multicast and
    /// answers by unicast, which is how stock covers one-way multicast.
    pub multicast_tx: bool,
}

impl Default for AutoConfig {
    fn default() -> Self {
        Self {
            group_id: DEFAULT_GROUP.to_vec(),
            scope: Scope::Link,
            addr_type: AddrType::Temporary,
            discovery_port: DISCOVERY_PORT,
            data_port: DATA_PORT,
            devices: Vec::new(),
            ignored_devices: Vec::new(),
            ifac: None,
            mode: InterfaceMode::Full,
            policy: IfacePolicy::default(),
            strict: true,
            multicast_tx: true,
        }
    }
}

/// A host interface chosen for discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adopted {
    pub name: String,
    pub index: u32,
    /// The address tokens are hashed over and the data socket binds.
    pub link_local: Ipv6Addr,
    /// Every link-local of the interface; tokens from any of them are our own echoes.
    pub all: Vec<Ipv6Addr>,
}

/// Choose the interfaces to adopt, in stock's order of checks (`AutoInterface.py`
/// 215-249): platform skip lists, `ignored_devices`, `lo0`, `devices`, then the last
/// `fe80::` address of each.
pub fn select(interfaces: &[NetIf], cfg: &AutoConfig) -> Vec<Adopted> {
    let darwin = cfg!(any(target_os = "macos", target_os = "ios"));
    let android = cfg!(target_os = "android");
    let listed = |list: &[String], name: &str| list.iter().any(|n| n == name);
    let mut adopted = Vec::new();
    for netif in interfaces {
        let name = netif.name.as_str();
        let allowed = listed(&cfg.devices, name);
        let skipped = (darwin && DARWIN_IGNORE.contains(&name) && !allowed)
            || (android && ANDROID_IGNORE.contains(&name) && !allowed)
            || listed(&cfg.ignored_devices, name)
            || name == "lo0"
            || (!cfg.devices.is_empty() && !allowed)
            || (cfg.strict && !(netif.up && netif.multicast));
        if let (false, Some(&link_local)) = (skipped, netif.link_local.last()) {
            adopted.push(Adopted {
                name: netif.name.clone(),
                index: netif.index,
                link_local,
                all: netif.link_local.clone(),
            });
        }
    }
    adopted
}

/// The link-local an adopted interface keeps on a job: the current one while the interface
/// still has it, else its last. Stock re-picks every job, so an interface with two
/// link-locals flips between them every 4 s (`AutoInterface.py` 416-424).
pub(crate) fn keep_link_local(current: Ipv6Addr, now: &[Ipv6Addr]) -> Option<Ipv6Addr> {
    if now.contains(&current) {
        Some(current)
    } else {
        now.last().copied()
    }
}
