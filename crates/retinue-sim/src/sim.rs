//! The run: a shared-radio medium, a clock, and the nodes, driven event by event.

mod events;
mod perform;
mod report;
mod state;

use retinue::node::InterfaceId;

use crate::scenario::Scenario;
use crate::trace::Trace;
use state::Sim;

/// Every node has one radio.
const RADIO: InterfaceId = 0;

/// Why a scenario could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    DuplicateNode(String),
    /// A node name with a dot, which a destination aspect cannot carry.
    BadNodeName(String),
    UnknownNode(String),
    /// A cut names two nodes with no edge between them.
    NoSuchEdge(String, String),
    /// A `Node` call returned more actions than its `ACTIONS` bound held.
    ActionsOverflowed {
        node: String,
        t: u64,
    },
    /// A frame on the medium did not decode.
    Undecodable {
        frame: u32,
    },
    /// A timebase past the announce field's 40 bits.
    Timebase,
}

impl core::fmt::Display for SimError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SimError {}

/// Run with the T114 channel node's table bounds: 32 peers, 8 actions, 4 links, 16 routes.
pub fn run(scenario: &Scenario) -> Result<Trace, SimError> {
    run_with::<32, 8, 4, 16>(scenario)
}

/// Run with caller-chosen `Node` table bounds.
pub fn run_with<
    const PEERS: usize,
    const ACTIONS: usize,
    const LINKS: usize,
    const ROUTES: usize,
>(
    scenario: &Scenario,
) -> Result<Trace, SimError> {
    Sim::<PEERS, ACTIONS, LINKS, ROUTES>::new(scenario)?.run()
}
