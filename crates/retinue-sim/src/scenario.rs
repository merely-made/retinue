//! What a run is given: a topology, the cuts, the sends, and the clock's settings.

use serde::{Deserialize, Serialize};

/// One radio. Its identity and destination are derived from `name`, so a topology is data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSpec {
    pub name: String,
    /// Run with [`retinue::node::TransportConfig::transit`]: relay announces and packets.
    /// Every node learns routes either way.
    pub transit: bool,
}

/// Two radios in range of each other. Undirected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub a: String,
    pub b: String,
}

/// Who can hear whom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub nodes: Vec<NodeSpec>,
    pub edges: Vec<Edge>,
}

/// An edge goes silent in both directions at `at` and stays silent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cut {
    pub at: u64,
    pub a: String,
    pub b: String,
}

/// An application send: open a link from `from` to `to`, then carry `payload` on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Send {
    pub at: u64,
    pub from: String,
    pub to: String,
    pub payload: String,
}

/// Clock settings, all in milliseconds, the unit `Node` ticks in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    /// From the start of a transmission to its arrival at every hearer.
    pub hop_delay: u64,
    /// How often each node's timers are advanced.
    pub poll_interval: u64,
    /// Each node's re-announce cadence.
    pub announce_interval: u64,
    /// Nothing scheduled after this runs.
    pub end: u64,
}

impl Timing {
    /// Retinue's defaults, polled at the channel node's five-second beat.
    pub const fn defaults(end: u64) -> Self {
        Self {
            hop_delay: 100,
            poll_interval: 5_000,
            announce_interval: retinue::node::DEFAULT_ANNOUNCE_INTERVAL,
            end,
        }
    }
}

/// Everything one run needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub topology: Topology,
    pub cuts: Vec<Cut>,
    pub sends: Vec<Send>,
    pub timing: Timing,
}
