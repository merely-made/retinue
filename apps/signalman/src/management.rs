//! Pure management vocabulary and projection for Signalman's device graph.
//!
//! Postilion reports radio facts. This module gives those facts stable product
//! identities and labels without taking ownership of routing, link, or station
//! state. The desktop may persist and render the result; it must not infer facts
//! that are absent here.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use postilion::management::AnnounceKind;
pub use postilion::management::{ManagementGeneration, ManagementSnapshot};
use retinue::endpoint::{LinkDirection, LinkFactKind};
use retinue::hash::AddressHash;
use retinue::identity::Identity;

/// Stable identity of one node in Signalman's device-data graph.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManagementNodeId(String);

impl ManagementNodeId {
    /// Build an id from another source authority's stable namespaced key.
    pub fn from_source_key(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn destination(destination: AddressHash) -> Self {
        Self(format!("destination:{destination}"))
    }

    pub fn identity(identity: Identity) -> Self {
        Self(format!(
            "identity:{}",
            hex::encode(identity.to_public_bytes())
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ManagementNodeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Product roles derived from current radio facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagementRole {
    Station,
    Peer,
    PropagationNode,
    TransportRelay,
    KnownButStale,
}

/// What an announce actually decoded as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnnounceClassification {
    Delivery,
    Propagation,
    Unknown,
}

/// Whether any current fact still presents the node as live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagementPresence {
    Stale,
    Live,
}

/// Source authority for one imported graph fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagementProvenance {
    Station,
    Announce,
    Route,
    Link,
}

/// Source stamp retained on every imported node and relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ManagementSource {
    pub generation: ManagementGeneration,
    pub observed_unix_ms: u64,
    pub provenance: ManagementProvenance,
    pub observation_sequence: Option<u64>,
}

/// One desired graph node before Chartulary reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagementNode {
    pub id: ManagementNodeId,
    pub label: String,
    pub roles: BTreeSet<ManagementRole>,
    pub announce_classes: BTreeSet<AnnounceClassification>,
    pub presence: ManagementPresence,
    pub source: ManagementSource,
}

/// Stable identity of one source relation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManagementRelationId(String);

impl ManagementRelationId {
    /// Build an id from a source relation's stable key.
    pub fn from_source_key(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ManagementRelationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Open Signalman relation vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagementRelationKind {
    HeardAnnounce,
    RouteVia,
    LiveLink,
}

impl ManagementRelationKind {
    pub const fn vocabulary(self) -> &'static str {
        match self {
            Self::HeardAnnounce => "signalman:heard-announce",
            Self::RouteVia => "signalman:route-via",
            Self::LiveLink => "signalman:live-link",
        }
    }
}

/// One desired graph relation before Chartulary reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagementRelation {
    pub id: ManagementRelationId,
    pub from: ManagementNodeId,
    pub to: ManagementNodeId,
    pub kind: ManagementRelationKind,
    pub label: String,
    pub source: ManagementSource,
}

/// Deterministic desired material for the device-data mere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagementMaterial {
    pub generation: ManagementGeneration,
    pub captured_unix_ms: u64,
    pub stale_after_ms: u64,
    pub nodes: Vec<ManagementNode>,
    pub relations: Vec<ManagementRelation>,
}

/// Owner-configurable age after which an observation is described as stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StalePolicy {
    pub after: Duration,
}

impl Default for StalePolicy {
    fn default() -> Self {
        Self {
            after: Duration::from_secs(15 * 60),
        }
    }
}

/// Project one Postilion snapshot into stable, deterministic graph material.
pub fn project_management(
    snapshot: &ManagementSnapshot,
    captured_unix_ms: u64,
    stale: StalePolicy,
) -> ManagementMaterial {
    let station_id = ManagementNodeId::destination(snapshot.station.delivery_destination);
    let station_source = source(
        snapshot.generation,
        captured_unix_ms,
        Duration::ZERO,
        ManagementProvenance::Station,
        None,
    );
    let mut nodes = BTreeMap::new();
    merge_node(
        &mut nodes,
        ManagementNode {
            id: station_id.clone(),
            label: snapshot.station.name.clone(),
            roles: BTreeSet::from([ManagementRole::Station]),
            announce_classes: BTreeSet::new(),
            presence: ManagementPresence::Live,
            source: station_source,
        },
    );

    let mut relations = BTreeMap::new();
    for observation in &snapshot.current_announces {
        let id = ManagementNodeId::destination(observation.fact.destination);
        let classification = match &observation.kind {
            AnnounceKind::Delivery(_) => AnnounceClassification::Delivery,
            AnnounceKind::Propagation(_) => AnnounceClassification::Propagation,
            AnnounceKind::Unknown => AnnounceClassification::Unknown,
        };
        let is_stale = observation.age >= stale.after;
        let mut roles = BTreeSet::from([ManagementRole::Peer]);
        if classification == AnnounceClassification::Propagation {
            roles.insert(ManagementRole::PropagationNode);
        }
        if is_stale {
            roles.insert(ManagementRole::KnownButStale);
        }
        let label = match &observation.kind {
            AnnounceKind::Delivery(delivery) => delivery
                .display_name
                .as_deref()
                .and_then(|name| std::str::from_utf8(name).ok())
                .map(str::to_owned)
                .unwrap_or_else(|| short_destination(observation.fact.destination)),
            AnnounceKind::Propagation(_) => format!(
                "Propagation {}",
                short_destination(observation.fact.destination)
            ),
            AnnounceKind::Unknown => format!(
                "Unknown {}",
                short_destination(observation.fact.destination)
            ),
        };
        let stamp = source(
            snapshot.generation,
            captured_unix_ms,
            observation.age,
            ManagementProvenance::Announce,
            Some(observation.fact.sequence),
        );
        merge_node(
            &mut nodes,
            ManagementNode {
                id: id.clone(),
                label,
                roles,
                announce_classes: BTreeSet::from([classification]),
                presence: if is_stale {
                    ManagementPresence::Stale
                } else {
                    ManagementPresence::Live
                },
                source: stamp,
            },
        );
        let relation = ManagementRelation {
            // The sequence is payload, not identity: a refreshed announce must not look like
            // new topology to Chartulary or the layout worker.
            id: ManagementRelationId(format!("announce:{}", observation.fact.destination)),
            from: station_id.clone(),
            to: id,
            kind: ManagementRelationKind::HeardAnnounce,
            label: format!(
                "{} hop{} on interface {}",
                observation.fact.hops,
                if observation.fact.hops == 1 { "" } else { "s" },
                observation.fact.interface
            ),
            source: stamp,
        };
        relations.insert(relation.id.clone(), relation);

        if let Some(transport) = observation.fact.transport {
            let transport_id = ManagementNodeId::destination(transport);
            merge_node(
                &mut nodes,
                relay_node(transport_id.clone(), transport, stamp, is_stale),
            );
            let relation = ManagementRelation {
                id: ManagementRelationId(format!(
                    "announce-route:{}:{}",
                    observation.fact.destination, transport
                )),
                from: ManagementNodeId::destination(observation.fact.destination),
                to: transport_id,
                kind: ManagementRelationKind::RouteVia,
                label: format!("heard via {}", short_destination(transport)),
                source: stamp,
            };
            relations.insert(relation.id.clone(), relation);
        }
    }

    for route in &snapshot.routes {
        let destination = ManagementNodeId::destination(route.destination);
        let is_stale = route.age >= stale.after;
        let stamp = source(
            snapshot.generation,
            captured_unix_ms,
            route.age,
            ManagementProvenance::Route,
            None,
        );
        merge_node(
            &mut nodes,
            ManagementNode {
                id: destination.clone(),
                label: short_destination(route.destination),
                roles: if is_stale {
                    BTreeSet::from([ManagementRole::KnownButStale])
                } else {
                    BTreeSet::new()
                },
                announce_classes: BTreeSet::new(),
                presence: if is_stale {
                    ManagementPresence::Stale
                } else {
                    ManagementPresence::Live
                },
                source: stamp,
            },
        );
        let (to, qualifier) = if let Some(transport) = route.transport {
            let transport_id = ManagementNodeId::destination(transport);
            merge_node(
                &mut nodes,
                relay_node(transport_id.clone(), transport, stamp, is_stale),
            );
            (transport_id, format!(" via {transport}"))
        } else {
            (station_id.clone(), String::new())
        };
        let relation = ManagementRelation {
            id: ManagementRelationId(format!(
                "route:{}:{}",
                route.destination,
                route
                    .transport
                    .map(|transport| transport.to_string())
                    .unwrap_or_else(|| "direct".to_owned())
            )),
            from: destination,
            to,
            kind: ManagementRelationKind::RouteVia,
            label: format!(
                "{} hop{} on interface {}{qualifier}",
                route.hops,
                if route.hops == 1 { "" } else { "s" },
                route.interface
            ),
            source: stamp,
        };
        relations.insert(relation.id.clone(), relation);
    }

    for link in &snapshot.links {
        let remote = link
            .remote
            .destination
            .map(ManagementNodeId::destination)
            .or_else(|| {
                link.remote.identity.and_then(|identity| {
                    snapshot
                        .current_announces
                        .iter()
                        .find(|observation| observation.fact.identity == identity)
                        .map(|observation| {
                            ManagementNodeId::destination(observation.fact.destination)
                        })
                })
            })
            .or_else(|| link.remote.identity.map(ManagementNodeId::identity));
        let Some(remote) = remote else {
            continue;
        };
        let stamp = source(
            snapshot.generation,
            captured_unix_ms,
            Duration::ZERO,
            ManagementProvenance::Link,
            None,
        );
        merge_node(
            &mut nodes,
            ManagementNode {
                id: remote.clone(),
                label: remote.as_str().to_owned(),
                roles: BTreeSet::from([ManagementRole::Peer]),
                announce_classes: BTreeSet::new(),
                presence: ManagementPresence::Live,
                source: stamp,
            },
        );
        let (from, to) = match link.direction {
            LinkDirection::Inbound => (remote, station_id.clone()),
            LinkDirection::Outbound => (station_id.clone(), remote),
        };
        let relation = ManagementRelation {
            id: ManagementRelationId(format!("link:{}", link.id)),
            from,
            to,
            kind: ManagementRelationKind::LiveLink,
            label: format!("{} on interface {}", link_kind(link.kind), link.interface),
            source: stamp,
        };
        relations.insert(relation.id.clone(), relation);
    }

    let nodes = nodes.into_values().collect::<Vec<_>>();
    let relations = relations.into_values().collect::<Vec<_>>();
    debug_assert!(relations.iter().all(|relation| {
        nodes.iter().any(|node| node.id == relation.from)
            && nodes.iter().any(|node| node.id == relation.to)
    }));
    ManagementMaterial {
        generation: snapshot.generation,
        captured_unix_ms,
        stale_after_ms: stale.after.as_millis().min(u128::from(u64::MAX)) as u64,
        nodes,
        relations,
    }
}

fn source(
    generation: ManagementGeneration,
    captured_unix_ms: u64,
    age: Duration,
    provenance: ManagementProvenance,
    observation_sequence: Option<u64>,
) -> ManagementSource {
    let age_ms = age.as_millis().min(u128::from(u64::MAX)) as u64;
    ManagementSource {
        generation,
        observed_unix_ms: captured_unix_ms.saturating_sub(age_ms),
        provenance,
        observation_sequence,
    }
}

fn relay_node(
    id: ManagementNodeId,
    destination: AddressHash,
    source: ManagementSource,
    is_stale: bool,
) -> ManagementNode {
    ManagementNode {
        id,
        label: format!("Relay {}", short_destination(destination)),
        roles: if is_stale {
            BTreeSet::from([
                ManagementRole::TransportRelay,
                ManagementRole::KnownButStale,
            ])
        } else {
            BTreeSet::from([ManagementRole::TransportRelay])
        },
        announce_classes: BTreeSet::new(),
        presence: if is_stale {
            ManagementPresence::Stale
        } else {
            ManagementPresence::Live
        },
        source,
    }
}

fn merge_node(nodes: &mut BTreeMap<ManagementNodeId, ManagementNode>, incoming: ManagementNode) {
    match nodes.get_mut(&incoming.id) {
        None => {
            nodes.insert(incoming.id.clone(), incoming);
        }
        Some(current) => {
            current.roles.extend(incoming.roles);
            current.announce_classes.extend(incoming.announce_classes);
            if incoming.presence == ManagementPresence::Live {
                current.presence = ManagementPresence::Live;
                current.roles.remove(&ManagementRole::KnownButStale);
            }
            // Projection order is semantic: decoded labels arrive before generic route/link
            // labels, so a fresher route makes the node live without erasing the display name.
            if current.label.is_empty() && !incoming.label.is_empty() {
                current.label = incoming.label;
            }
            if incoming.source.observed_unix_ms >= current.source.observed_unix_ms {
                current.source = incoming.source;
            }
        }
    }
}

fn short_destination(destination: AddressHash) -> String {
    destination.to_string().chars().take(8).collect()
}

fn link_kind(kind: LinkFactKind) -> &'static str {
    match kind {
        LinkFactKind::BestEffort => "best-effort link",
        LinkFactKind::Reliable => "reliable link",
        LinkFactKind::Resource => "resource link",
    }
}

#[cfg(test)]
mod tests;
