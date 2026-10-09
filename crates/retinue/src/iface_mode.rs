//! The seven RNS interface modes and their announce and path-request rules (`Interface.py` 45-56).

/// How an interface's peers come and go: RNS's interface modes (`Interface.py` 45-51). A mode
/// bounds route lifetime and decides which announces may leave on the interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum InterfaceMode {
    /// Peers are stable: routes live for the configured route TTL.
    #[default]
    Full,
    /// A link to exactly one peer; routes as [`Self::Full`].
    PointToPoint,
    /// Peers are clients that come and go: routes live at most a day, and no announce is
    /// broadcast to them, this node's own included.
    AccessPoint,
    /// This node moves between peers: routes live at most six hours.
    Roaming,
    /// The edge between two networks.
    Boundary,
    /// A gateway into a network that should discover paths on behalf of its clients.
    Gateway,
    /// A network internal to this instance.
    Internal,
}

impl InterfaceMode {
    /// Every mode, for searches that do not filter by mode.
    pub const ALL: [Self; 7] = [
        Self::Full,
        Self::PointToPoint,
        Self::AccessPoint,
        Self::Roaming,
        Self::Boundary,
        Self::Gateway,
        Self::Internal,
    ];
}

/// The INTERNAL-mode switches that bear on one announce's broadcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeFlags {
    /// The egress interface's `announces_from_internal`: whether announces learned on an
    /// INTERNAL interface may leave on it.
    pub from_internal: bool,
    /// The next-hop interface's `announces_to_internal`: whether announces learned there may
    /// reach INTERNAL interfaces even from a BOUNDARY one.
    pub to_internal: bool,
}

impl Default for ModeFlags {
    /// RNS's defaults (`Interface.py` 75-130).
    fn default() -> Self {
        Self {
            from_internal: true,
            to_internal: false,
        }
    }
}

/// Whether an announce broadcast with no attached interface may go out on an `out`-mode
/// interface (`Transport.py` 1458-1516). `from` is the mode of the interface its route was
/// learned on, `None` if that interface is gone; `local` marks this instance's own
/// destination. A path response, sent on one interface, is not subject to these rules.
pub const fn announce_permitted(
    out: InterfaceMode,
    from: Option<InterfaceMode>,
    local: bool,
    flags: ModeFlags,
) -> bool {
    use InterfaceMode::{AccessPoint, Boundary, Internal, Roaming};
    if local {
        return !matches!(out, AccessPoint);
    }
    let Some(from) = from else {
        return false;
    };
    if !flags.from_internal && matches!(from, Internal) {
        return false;
    }
    match out {
        AccessPoint => false,
        Internal => flags.to_internal || !matches!(from, Boundary),
        Roaming => !matches!(from, Roaming | Boundary),
        Boundary => !matches!(from, Roaming),
        _ => true,
    }
}

/// The modes a transport node searches when a path request for an unknown destination arrives
/// on a `mode` interface, or `None` if it does not search (`Interface.py` 55-56;
/// `Transport.py` 3426-3433, 3574-3575).
pub const fn discovers_paths(mode: InterfaceMode) -> Option<&'static [InterfaceMode]> {
    use InterfaceMode::{AccessPoint, Boundary, Gateway, Internal, Roaming};
    match mode {
        AccessPoint | Gateway | Roaming | Internal => Some(&InterfaceMode::ALL),
        Boundary => Some(&[Boundary, Gateway]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::InterfaceMode::*;
    use super::*;

    const FLAGS: ModeFlags = ModeFlags {
        from_internal: true,
        to_internal: false,
    };

    fn relay(out: InterfaceMode, from: InterfaceMode) -> bool {
        announce_permitted(out, Some(from), false, FLAGS)
    }

    #[test]
    fn own_announces_skip_only_access_points() {
        for out in InterfaceMode::ALL {
            assert_eq!(
                announce_permitted(out, None, true, FLAGS),
                out != AccessPoint
            );
        }
    }

    #[test]
    fn relays_follow_the_mode_matrix() {
        for from in InterfaceMode::ALL {
            assert!(!relay(AccessPoint, from));
            assert_eq!(relay(Roaming, from), !matches!(from, Roaming | Boundary));
            assert_eq!(relay(Boundary, from), from != Roaming);
            assert_eq!(relay(Internal, from), from != Boundary);
            for out in [Full, PointToPoint, Gateway] {
                assert!(relay(out, from));
            }
        }
        assert!(!announce_permitted(Full, None, false, FLAGS), "no next hop");
    }

    #[test]
    fn internal_flags_open_and_close_their_paths() {
        let to_internal = ModeFlags {
            to_internal: true,
            ..FLAGS
        };
        assert!(announce_permitted(
            Internal,
            Some(Boundary),
            false,
            to_internal
        ));
        let closed = ModeFlags {
            from_internal: false,
            ..FLAGS
        };
        assert!(!announce_permitted(Full, Some(Internal), false, closed));
        assert!(announce_permitted(Full, Some(Gateway), false, closed));
        assert!(
            announce_permitted(Full, Some(Internal), true, closed),
            "own announces"
        );
    }

    #[test]
    fn discovery_modes() {
        assert_eq!(discovers_paths(Full), None);
        assert_eq!(discovers_paths(PointToPoint), None);
        assert_eq!(discovers_paths(Gateway), Some(&InterfaceMode::ALL[..]));
        assert_eq!(discovers_paths(Boundary), Some(&[Boundary, Gateway][..]));
    }
}
