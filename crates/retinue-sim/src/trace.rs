//! The trace schema. See the crate docs for its contract.

use serde::{Deserialize, Serialize};

use crate::scenario::{Cut, Edge, Send, Timing};

/// The schema id every trace carries. A breaking change takes a new version.
pub const SCHEMA: &str = "retinue-sim.route-trace/v1";

/// One run, from first poll to `timing.end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub schema: String,
    pub scenario: String,
    pub profile: Profile,
    pub timing: Timing,
    pub nodes: Vec<NodeInfo>,
    pub edges: Vec<Edge>,
    pub cuts: Vec<Cut>,
    pub sends: Vec<Send>,
    pub events: Vec<Event>,
    pub messages: Vec<Message>,
}

impl Trace {
    /// The canonical serialization: compact JSON, fields in declaration order.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("trace types serialize")
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }
}

/// The `Node` table bounds every node in the run was built with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub peers: u32,
    pub actions: u32,
    pub links: u32,
    pub routes: u32,
}

/// A node's derived addresses, as lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub name: String,
    pub destination: String,
    pub identity: String,
    pub transit: bool,
}

/// One step. Times are simulated milliseconds; events are in the order they happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// An edge went silent.
    Cut { t: u64, a: String, b: String },
    /// An application send began: the link request is about to be transmitted.
    Send {
        t: u64,
        message: u32,
        from: String,
        to: String,
        /// The first relay the request is addressed to, or `None` when sent direct.
        via: Option<String>,
        /// The route's hop count, when the sender has a route.
        hops: Option<u8>,
    },
    /// The sender's node would not open a link.
    SendRefused {
        t: u64,
        message: u32,
        from: String,
        to: String,
        reason: Refusal,
    },
    /// A node put a frame on the air.
    Transmit {
        t: u64,
        frame: u32,
        node: String,
        origin: Origin,
        /// The frame whose arrival produced this one, for `reply` and `forward`.
        cause: Option<u32>,
        packet: PacketSummary,
        /// Neighbours that will hear it, in topology order.
        heard_by: Vec<String>,
        /// Neighbours behind a cut edge, who will not.
        blocked: Vec<String>,
        state: NodeState,
    },
    /// A node heard a frame and its `Node` ingested it.
    Receive {
        t: u64,
        frame: u32,
        node: String,
        effects: Vec<Effect>,
        state: NodeState,
    },
    /// A link request this node opened got no proof by its deadline, and its `Node` dropped
    /// it (`Action::LinkRequestTimedOut`), from a poll or from inside `open_link`.
    LinkRequestExpired {
        t: u64,
        node: String,
        /// The request's link id.
        link: String,
        /// The send the request was opened for.
        message: Option<u32>,
        state: NodeState,
    },
    /// A message's payload reached its destination's application.
    Delivered {
        t: u64,
        message: u32,
        node: String,
        /// The data frame's path, sender first, following forwards only.
        path: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The sender has not heard the destination announce.
    UnknownDestination,
    /// The sender's pending-link table is full.
    PendingFull,
}

/// Why a node transmitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Its own announce, from a poll.
    Announce,
    /// The application: a link request or link data.
    App,
    /// The node's answer to a frame it received, such as a link proof.
    Reply,
    /// The received frame itself, relayed.
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketKind {
    Announce,
    LinkRequest,
    Proof,
    Data,
}

/// The header facts of a frame. Hashes are lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketSummary {
    pub packet_type: PacketKind,
    /// 1 or 2: whether the frame names a transport node.
    pub header: u8,
    pub hops: u8,
    pub destination: String,
    /// The node whose destination hash this is, if any. A link id names no node.
    pub destination_node: Option<String>,
    /// The transport node the frame is addressed through, by name.
    pub transport: Option<String>,
    pub context: u8,
    /// Encoded length: the frame's bytes on the air.
    pub len: u16,
    pub hash: String,
}

/// What one ingest did, from the `Action`s it returned. Sends appear as `transmit` events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    Learned {
        destination: String,
    },
    LinkUp {
        link: String,
    },
    LinkDown {
        link: String,
    },
    Data {
        link: String,
        len: u32,
        message: Option<u32>,
    },
    Resource {
        link: String,
        len: u32,
    },
}

/// One node's local state after an event: what its own face would show.
///
/// It maps onto radio-face as the Retinue channel node fills it
/// (`radio-hand/src/channel/node.rs` and `face.rs`):
/// `tx_frames`, `rx_frames` -> `LocalStatus.tx_frames`, `.rx_frames`;
/// `last_tx_len` -> `LocalStatus.last_tx = TxResult::Sent { frame_len }`;
/// `last_rx_len` -> `LocalStatus.last_rx.frame_len` (no RSSI or SNR is simulated);
/// `links` -> `HostSnapshot.link_count` and `.admitted_links`;
/// `event` -> `HostSnapshot.event`, with `EventSource::Local`.
/// `HostSnapshot.queue_depth` is the channel's unsent count, which the medium never
/// produces, so it is zero.
///
/// The mapping ships as `face::face`, behind the `face` feature, so a consumer does not
/// re-derive it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeState {
    pub tx_frames: u32,
    pub rx_frames: u32,
    pub last_tx_len: Option<u16>,
    pub last_rx_len: Option<u16>,
    pub links: u32,
    pub pending_links: u32,
    pub peers: u32,
    /// `Node::next_hop` toward every other node it has a fresh route to, in topology order.
    pub routes: Vec<RouteState>,
    /// The face's last event line, as the channel node notes it.
    pub event: Option<FaceEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteState {
    pub to: String,
    /// The first relay, or `None` when the destination is heard directly.
    pub via: Option<String>,
    pub hops: u8,
}

/// Mirrors `radio_face::UiEvent`; `text` fits its 24 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaceEvent {
    pub kind: FaceEventKind,
    pub text: String,
}

/// Mirrors `radio_face::EventKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceEventKind {
    Info,
    Received,
    Transmitted,
    Delivered,
    Propagated,
    Failed,
}

/// A send's outcome at the end of the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: u32,
    pub from: String,
    pub to: String,
    pub sent_at: u64,
    /// The link opened for it, if the sender opened one.
    pub link: Option<String>,
    pub delivered: Option<Delivery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub t: u64,
    pub path: Vec<String>,
}
