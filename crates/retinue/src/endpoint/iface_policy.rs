//! Per-interface announce and transmit policy: the RNS `[interfaces]` keys that are not the
//! carrier itself (`Interface.py` 75-130; `Reticulum.py` 881-963, 1031-1063).
//!
//! The setters only store; the router and carriers read a snapshot through
//! [`Shared::iface_policy`]. An entry lives exactly as long as its interface.

use std::time::Duration;

use crate::announce_admission::AnnounceIngressPolicy;

use super::interface::InterfaceId;
use super::runtime::Endpoint;
use super::shared::Shared;

/// RNS refuses a configured bitrate below this (`Reticulum.py` 134, 929-931).
const MINIMUM_BITRATE: u64 = 5;

/// A destination announce-rate rule for announces received on one interface
/// (`Transport.py` 2303-2338). Defaults are RNS's (`Interface.py` 90-92).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceRate {
    /// Minimum interval between one destination's announces. Never zero.
    pub target: Duration,
    /// Violations tolerated before the destination is blocked.
    pub grace: u16,
    /// Extra block time once the grace is spent.
    pub penalty: Duration,
}

impl Default for AnnounceRate {
    fn default() -> Self {
        Self {
            target: Duration::from_secs(3600),
            grace: 5,
            penalty: Duration::ZERO,
        }
    }
}

/// One interface's policy. The default is an RNS interface with nothing configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IfacePolicy {
    /// Ingress control override; `None` defers to the endpoint-wide policy.
    pub ingress: Option<AnnounceIngressPolicy>,
    /// Destination announce-rate rule; `None` defers to the endpoint-wide one.
    pub announce_rate: Option<AnnounceRate>,
    /// Share of the bitrate announces may use, in percent (`Reticulum.py` 114, 948-951).
    pub cap_percent: u8,
    /// Configured carrier bitrate; `None` keeps the carrier's own estimate.
    pub bitrate_bps: Option<u64>,
    /// Path preference for repeated emissions (`Transport.py` 2225-2251).
    pub gravity: i16,
    /// False forbids every transmission on this interface (`Transport.py` 1449).
    pub outgoing: bool,
    /// Whether announces learned on an INTERNAL interface are relayed out of this one
    /// (`Transport.py` 1471).
    pub announces_from_internal: bool,
    /// Whether announces learned here may be relayed to INTERNAL interfaces (`Transport.py` 1484).
    pub announces_to_internal: bool,
}

impl Default for IfacePolicy {
    fn default() -> Self {
        Self {
            ingress: None,
            announce_rate: None,
            cap_percent: 2,
            bitrate_bps: None,
            gravity: 0,
            outgoing: true,
            announces_from_internal: true,
            announces_to_internal: false,
        }
    }
}

impl Shared {
    /// The policy of interface `id`; the default if it is unset or not attached.
    #[allow(dead_code, reason = "read by the router and carriers as they land")]
    pub(super) fn iface_policy(&self, id: InterfaceId) -> IfacePolicy {
        let policies = self.iface_policies.lock().unwrap();
        policies.get(&id).copied().unwrap_or_default()
    }

    /// Apply `change` to an attached interface's policy. Holding the interface lock keeps a
    /// concurrent detach from leaving an orphan entry behind.
    fn update_iface_policy(&self, id: InterfaceId, change: impl FnOnce(&mut IfacePolicy)) -> bool {
        let interfaces = self.interfaces.lock().unwrap();
        if !interfaces.iter().any(|iface| iface.id == id) {
            return false;
        }
        change(self.iface_policies.lock().unwrap().entry(id).or_default());
        true
    }
}

/// Each setter returns false, storing nothing, if no such interface is attached or the value
/// is one RNS's config parser would refuse.
impl Endpoint {
    /// Override ingress control on one interface, or `None` to follow the endpoint's.
    pub fn set_interface_ingress_policy(
        &self,
        id: InterfaceId,
        policy: Option<AnnounceIngressPolicy>,
    ) -> bool {
        self.shared.update_iface_policy(id, |p| p.ingress = policy)
    }

    /// Set the announce-rate rule for announces received on one interface. A zero target is
    /// refused (`Reticulum.py` 933-946).
    pub fn set_interface_announce_rate(&self, id: InterfaceId, rate: Option<AnnounceRate>) -> bool {
        if rate.is_some_and(|r| r.target.is_zero()) {
            return false;
        }
        self.shared
            .update_iface_policy(id, |p| p.announce_rate = rate)
    }

    /// Set the share of an interface's bitrate announces may use, in percent: 1 to 100.
    pub fn set_announce_cap(&self, id: InterfaceId, percent: u8) -> bool {
        (1..=100).contains(&percent)
            && self
                .shared
                .update_iface_policy(id, |p| p.cap_percent = percent)
    }

    /// Configure an interface's bitrate, or `None` for the carrier's estimate. Below 5 bps
    /// is refused.
    pub fn set_interface_bitrate(&self, id: InterfaceId, bps: Option<u64>) -> bool {
        if bps.is_some_and(|b| b < MINIMUM_BITRATE) {
            return false;
        }
        self.shared.update_iface_policy(id, |p| p.bitrate_bps = bps)
    }

    /// Set an interface's gravity.
    pub fn set_interface_gravity(&self, id: InterfaceId, gravity: i16) -> bool {
        self.shared.update_iface_policy(id, |p| p.gravity = gravity)
    }

    /// Allow or forbid transmission on an interface; a receive-only interface still listens.
    pub fn set_interface_outgoing(&self, id: InterfaceId, outgoing: bool) -> bool {
        self.shared
            .update_iface_policy(id, |p| p.outgoing = outgoing)
    }

    /// Set the INTERNAL-mode announce flags: `from_internal` and `to_internal` as RNS's
    /// `announces_from_internal` and `announces_to_internal`.
    pub fn set_interface_mode_flags(
        &self,
        id: InterfaceId,
        from_internal: bool,
        to_internal: bool,
    ) -> bool {
        self.shared.update_iface_policy(id, |p| {
            p.announces_from_internal = from_internal;
            p.announces_to_internal = to_internal;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::PrivateIdentity;

    #[tokio::test]
    async fn setters_store_and_detach_forgets() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x51; 64]));
        let id = ep.attach_interface().id();
        let rate = AnnounceRate::default();
        assert!(ep.set_interface_ingress_policy(id, Some(AnnounceIngressPolicy::default())));
        assert!(ep.set_interface_announce_rate(id, Some(rate)));
        assert!(ep.set_announce_cap(id, 10));
        assert!(ep.set_interface_bitrate(id, Some(1200)));
        assert!(ep.set_interface_gravity(id, -3));
        assert!(ep.set_interface_outgoing(id, false));
        assert!(ep.set_interface_mode_flags(id, false, true));
        let refused = [
            ep.set_announce_cap(id, 0),
            ep.set_announce_cap(id, 101),
            ep.set_interface_bitrate(id, Some(4)),
            ep.set_interface_announce_rate(
                id,
                Some(AnnounceRate {
                    target: Duration::ZERO,
                    ..rate
                }),
            ),
        ];
        assert_eq!(
            refused, [false; 4],
            "RNS's config parser refuses these values"
        );
        assert_eq!(
            ep.shared.iface_policy(id),
            IfacePolicy {
                ingress: Some(AnnounceIngressPolicy::default()),
                announce_rate: Some(rate),
                cap_percent: 10,
                bitrate_bps: Some(1200),
                gravity: -3,
                outgoing: false,
                announces_from_internal: false,
                announces_to_internal: true,
            }
        );

        ep.detach_interface(id);
        assert!(ep.shared.iface_policies.lock().unwrap().is_empty());
        assert_eq!(ep.shared.iface_policy(id), IfacePolicy::default());
        assert!(
            !ep.set_interface_gravity(id, 1),
            "a detached interface takes no policy"
        );
        assert!(ep.shared.iface_policies.lock().unwrap().is_empty());
    }
}
