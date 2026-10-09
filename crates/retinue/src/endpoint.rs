//! The endpoint runtime: the tokio shell that turns the R0–R4 primitives into a working
//! peer.
//!
//! An [`Endpoint`] holds an identity, an address book, and any number of interfaces. A
//! background router reads packets from every interface, tagged with the interface they
//! arrived on, and dispatches them: announces populate the address book, inbound link
//! requests are proved and surfaced as connections, and link data reaches the
//! [`LinkStream`] for its link. This is the seam a host implements its own transport trait
//! against; see the crate root.

mod announces;
mod attach;
mod config;
mod dedup;
mod dial;
mod entropy;
mod facts;
mod iface_policy;
mod inbound;
mod interface;
mod known_destinations;
mod link_receipt;
mod link_setup;
mod listen;
mod paths;
mod pump;
mod queue;
mod rebroadcast;
mod registration;
mod reliable_driver;
mod resource_inbound;
mod resource_pace;
mod resource_requests;
mod resource_session;
mod router;
mod routing;
mod runtime;
mod sealing;
mod shared;
mod single;
mod sockopt;
mod stream;
#[cfg(test)]
mod tests;
mod transit;
mod watchdog;

pub use announces::AnnounceFreshnessPolicy;
pub use dial::TcpClient;
pub use facts::{
    AnnounceFact, EndpointFacts, LinkDirection, LinkFact, LinkFactKind, LinkRemoteFact,
    PeerAnnounce, RouteFact,
};
pub use iface_policy::{AnnounceRate, IfacePolicy};
pub use inbound::{Accepted, AcceptedResource, InboundLinkLimits};
pub use interface::{Interface, InterfaceId, InterfaceSink};
pub use link_receipt::{LinkDelivery, PayloadReceipt};
pub use listen::{ListenPolicy, Listener};
pub use queue::{
    ClassCounters, OutboundPackets, QueueCounters, QueueDepths, QueueWeights, TrafficClass,
};
pub use resource_inbound::{ReceivedLinkData, SessionInbound};
pub use resource_requests::{ReceivedRawRequest, ReceivedRawResponse, ReceivedRequest};
pub use resource_session::{PayloadMode, ReceivedPayload, ResourceSession, ResourceTransferConfig};
pub use routing::{InterfaceSelector, RoutingCounters, RoutingPolicy};
pub use runtime::Endpoint;
pub use single::{ProofStrategy, ReceivedSingle, SingleDelivery, SinglePacketReceipt};
pub use stream::LinkStream;
