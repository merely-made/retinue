//! Runtime tunables and their defaults.

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::channel::StreamDecodeLimitError;

use super::interface::InterfaceId;
use super::runtime::Endpoint;

/// Fast interfaces start here; radio callers can raise it before opening links.
pub(super) const DEFAULT_RELIABLE_INITIAL_RTT_MS: u64 = 750;

/// Default dynamic Channel ceiling, matching RNS. Strict half-duplex callers
/// can lower it without changing the wire format.
pub(super) const DEFAULT_RELIABLE_MAX_WINDOW: u32 = crate::channel::WINDOW_MAX;

/// Default link MTU advertised by Reticulum; radio callers may lower it.
pub(super) const DEFAULT_LINK_MTU: u32 = crate::packet::MTU as u32;
/// Smallest link MTU the direct-PHY Data and Resource paths exercise: room for an
/// eight-byte IFAC on a 255-byte packet radio.
const MIN_LINK_MTU: u32 = 247;

/// Default interval between identical link-request transmissions while setup is pending.
pub(super) const DEFAULT_LINK_SETUP_RETRY_MS: u64 = 2_000;

impl Endpoint {
    /// Spread announce relays over a random delay of `0..=max`, so neighbours relaying the
    /// same announce on a shared medium do not transmit simultaneously.
    ///
    /// Off by default: it costs latency and buys nothing point to point. On a shared radio,
    /// set it near the air time of an announce.
    pub fn set_relay_jitter(&self, max: Duration) {
        let ms = max.as_millis().min(u128::from(u64::MAX)) as u64;
        self.shared.relay_jitter_ms.store(ms, Ordering::Relaxed);
    }

    /// Set the first reliable-channel RTT estimate for subsequently opened links. It is a
    /// floor under the RTT an initiator measures at setup, and a responder's whole estimate.
    /// Slow half-duplex radios should include their queue and proof turnaround time.
    pub fn set_reliable_initial_rtt(&self, rtt: Duration) {
        let millis = rtt.as_millis().clamp(1, u128::from(u64::MAX)) as u64;
        self.shared
            .reliable_initial_rtt_ms
            .store(millis, Ordering::Relaxed);
    }

    /// Cap the reliable Channel send window for subsequently opened links.
    ///
    /// Set this to one on strict half-duplex media so each data frame is proved
    /// before another transmission begins. The default is RNS's dynamic maximum.
    pub fn set_reliable_max_window(&self, frames: u32) {
        self.shared.reliable_max_window.store(
            frames.clamp(1, crate::channel::WINDOW_MAX),
            Ordering::Relaxed,
        );
    }

    /// Set the decoded output ceiling for each compressed frame on subsequently
    /// opened reliable streams. This excludes the bz2 decoder's own workspace.
    pub fn set_reliable_decoded_frame_limit(
        &self,
        limit: usize,
    ) -> Result<(), StreamDecodeLimitError> {
        if limit == 0 || limit >= isize::MAX as usize {
            return Err(StreamDecodeLimitError::InvalidLimit);
        }
        self.shared
            .reliable_decoded_frame_limit
            .store(limit, Ordering::Relaxed);
        Ok(())
    }

    /// Set the retry interval for link requests sent by subsequently opened links.
    pub fn set_link_setup_retry(&self, interval: Duration) {
        let millis = interval.as_millis().clamp(1, u128::from(u64::MAX)) as u64;
        self.shared
            .link_setup_retry_ms
            .store(millis, Ordering::Relaxed);
    }

    /// Set the first-hop airtime allowance added to the setup deadline of links opened over
    /// `iface`: the time its medium needs to carry one 500-byte MTU
    /// ([`crate::node::first_hop_airtime`] computes it from a bitrate). Zero, the default,
    /// suits an unbounded medium such as TCP.
    pub fn set_first_hop_airtime(&self, iface: InterfaceId, allowance: Duration) {
        let millis = allowance.as_millis().min(u128::from(u64::MAX)) as u64;
        let mut airtime = self.shared.first_hop_airtime_ms.lock().unwrap();
        if millis == 0 {
            airtime.remove(&iface);
        } else {
            airtime.insert(iface, millis);
        }
    }

    /// Set the MTU requested and offered by subsequently established links.
    ///
    /// The lower bound keeps link setup, identify, and resource control packets
    /// representable while allowing the standard eight-byte IFAC on a 255-byte
    /// packet radio. The default remains Reticulum's 500-byte MTU.
    pub fn set_link_mtu(&self, mtu: u32) {
        self.shared.link_mtu.store(
            mtu.clamp(MIN_LINK_MTU, crate::packet::MTU as u32),
            Ordering::Relaxed,
        );
    }
}
