//! Signalman's app-owned, logged device graph.
//!
//! Stable radio identities are the graph identities. Chartulary is the only
//! node and relation registry; views consume [`DeviceProjection`] rather than
//! rebuilding a second network-shaped cache.

use std::collections::BTreeMap;

use chartulary::{Author, EdgeId, EditSpec, GraphLog, Identified, NodeKey};
use signalman::management::{
    ManagementGeneration, ManagementMaterial, ManagementNode, ManagementNodeId, ManagementPresence,
    ManagementRelation, ManagementRelationId, ManagementRole,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceNode(pub ManagementNode);

impl Identified for DeviceNode {
    type Id = ManagementNodeId;

    fn id(&self) -> &Self::Id {
        &self.0.id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRelation(pub ManagementRelation);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedNode {
    pub key: NodeKey,
    pub fact: ManagementNode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedRelation {
    pub id: ManagementRelationId,
    pub from: NodeKey,
    pub to: NodeKey,
    pub fact: ManagementRelation,
}

/// One canonical graph projection shared by layout, canvas, and companion list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceProjection {
    pub topology_epoch: u64,
    pub nodes: Vec<ProjectedNode>,
    pub relations: Vec<ProjectedRelation>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReceipt {
    pub graph_edits: usize,
    pub topology_changed: bool,
    pub revision: u64,
    pub topology_epoch: u64,
}

pub struct DeviceMere {
    graph: GraphLog<DeviceNode, DeviceRelation>,
    // Chartulary exposes retraction by EdgeId but does not yet expose an
    // EdgeId iterator. This is only the app-local retraction index; relation
    // payloads remain authoritative in the logged graph.
    edge_ids: BTreeMap<ManagementRelationId, EdgeId>,
    generation: Option<ManagementGeneration>,
    selected: Option<ManagementNodeId>,
    topology_epoch: u64,
}

impl Default for DeviceMere {
    fn default() -> Self {
        Self {
            graph: GraphLog::new(),
            edge_ids: BTreeMap::new(),
            generation: None,
            selected: None,
            topology_epoch: 0,
        }
    }
}

impl DeviceMere {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn graph(&self) -> &GraphLog<DeviceNode, DeviceRelation> {
        &self.graph
    }

    pub fn generation(&self) -> Option<ManagementGeneration> {
        self.generation
    }

    pub fn selected(&self) -> Option<&ManagementNodeId> {
        self.selected.as_ref()
    }

    pub fn select(&mut self, id: Option<ManagementNodeId>) -> bool {
        let accepted = id
            .as_ref()
            .is_none_or(|id| self.graph.graph().key_of(id).is_some());
        if accepted {
            self.selected = id;
        }
        accepted
    }

    /// Reconcile desired source material as one attributed Chartulary batch.
    /// An empty diff appends no batch.
    pub fn reconcile(&mut self, material: &ManagementMaterial) -> ReconcileReceipt {
        let desired_nodes: BTreeMap<_, _> = material
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect();
        let desired_relations: BTreeMap<_, _> = material
            .relations
            .iter()
            .cloned()
            .map(|relation| (relation.id.clone(), relation))
            .collect();
        let current_relations = self.current_relations();

        let mut specs = Vec::new();
        let mut topology_changed = false;

        // Remove changed or disappeared relation instances first. Their endpoint
        // nodes remain as retained, stale device history.
        let removed_relations: Vec<_> = current_relations
            .iter()
            .filter(|(id, current)| desired_relations.get(*id) != Some(*current))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &removed_relations {
            if let Some(edge_id) = self.edge_ids.get(id).copied() {
                specs.push(EditSpec::Disconnect(edge_id));
                let current = self
                    .current_relation(id)
                    .expect("removed relation was present");
                if desired_relations
                    .get(id)
                    .is_none_or(|desired| !same_topology(&current, desired))
                {
                    topology_changed = true;
                }
            }
        }

        let existing_nodes: BTreeMap<_, _> = self
            .graph
            .graph()
            .nodes()
            .map(|(_, node)| (node.0.id.clone(), node.0.clone()))
            .collect();
        for (id, desired) in &desired_nodes {
            match existing_nodes.get(id) {
                Some(current) if current == desired => {}
                Some(_) => specs.push(EditSpec::InsertNode(DeviceNode(desired.clone()))),
                None => {
                    specs.push(EditSpec::InsertNode(DeviceNode(desired.clone())));
                    topology_changed = true;
                }
            }
        }

        // Absence retains the last proven source stamp. It becomes stale only
        // when the caller's captured time crosses the configured threshold;
        // unrelated source generations therefore cannot churn retained facts.
        for (id, current) in &existing_nodes {
            if desired_nodes.contains_key(id) {
                continue;
            }
            let elapsed = material
                .captured_unix_ms
                .saturating_sub(current.source.observed_unix_ms);
            if elapsed < material.stale_after_ms {
                continue;
            }
            let mut retained = current.clone();
            retained.presence = ManagementPresence::Stale;
            retained.roles.insert(ManagementRole::KnownButStale);
            if &retained != current {
                specs.push(EditSpec::InsertNode(DeviceNode(retained)));
            }
        }

        let mut connected_ids = Vec::new();
        for (id, desired) in &desired_relations {
            if current_relations.get(id) == Some(desired) {
                continue;
            }
            specs.push(EditSpec::Connect {
                from: desired.from.clone(),
                to: desired.to.clone(),
                edge: DeviceRelation(desired.clone()),
            });
            connected_ids.push(id.clone());
            if current_relations
                .get(id)
                .is_none_or(|current| !same_topology(current, desired))
            {
                topology_changed = true;
            }
        }

        if specs.is_empty() {
            self.generation = Some(material.generation);
            return ReconcileReceipt {
                graph_edits: 0,
                topology_changed: false,
                revision: self.graph.revision(),
                topology_epoch: self.topology_epoch,
            };
        }

        let edit_count = specs.len();
        let committed = self
            .graph
            .commit_batch(
                Author::new("signalman:management-snapshot"),
                self.graph.revision(),
                specs,
            )
            .expect("single-writer reconciliation is based on the current revision");

        for id in removed_relations {
            self.edge_ids.remove(&id);
        }
        assert_eq!(connected_ids.len(), committed.edges.len());
        for (id, edge_id) in connected_ids.into_iter().zip(committed.edges) {
            self.edge_ids.insert(id.clone(), edge_id);
        }
        self.generation = Some(material.generation);
        if topology_changed {
            self.topology_epoch = self.topology_epoch.saturating_add(1);
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|id| self.graph.graph().key_of(id).is_none())
        {
            self.selected = None;
        }

        ReconcileReceipt {
            graph_edits: edit_count,
            topology_changed,
            revision: self.graph.revision(),
            topology_epoch: self.topology_epoch,
        }
    }

    pub fn projection(&self) -> DeviceProjection {
        let mut nodes: Vec<_> = self
            .graph
            .graph()
            .nodes()
            .map(|(key, node)| ProjectedNode {
                key,
                fact: node.0.clone(),
            })
            .collect();
        nodes.sort_by(|left, right| left.fact.id.cmp(&right.fact.id));

        let node_keys: BTreeMap<_, _> = nodes
            .iter()
            .map(|node| (node.fact.id.clone(), node.key))
            .collect();
        let mut relations = self
            .edge_ids
            .iter()
            .filter_map(|(id, edge_id)| {
                let edge_key = self.graph.edge_key(*edge_id)?;
                let relation = &self.graph.graph().edge(edge_key)?.0;
                Some(ProjectedRelation {
                    id: id.clone(),
                    from: *node_keys.get(&relation.from)?,
                    to: *node_keys.get(&relation.to)?,
                    fact: relation.clone(),
                })
            })
            .collect::<Vec<_>>();
        relations.sort_by(|left, right| left.id.cmp(&right.id));
        debug_assert_eq!(relations.len(), self.edge_ids.len());

        DeviceProjection {
            topology_epoch: self.topology_epoch,
            nodes,
            relations,
        }
    }

    fn current_relation(&self, id: &ManagementRelationId) -> Option<ManagementRelation> {
        let edge_id = self.edge_ids.get(id)?;
        let edge_key = self.graph.edge_key(*edge_id)?;
        Some(self.graph.graph().edge(edge_key)?.0.clone())
    }

    fn current_relations(&self) -> BTreeMap<ManagementRelationId, ManagementRelation> {
        self.edge_ids
            .keys()
            .filter_map(|id| Some((id.clone(), self.current_relation(id)?)))
            .collect()
    }
}

fn same_topology(left: &ManagementRelation, right: &ManagementRelation) -> bool {
    left.from == right.from && left.to == right.to && left.kind == right.kind
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::time::Duration;

    use signalman::management::{
        ManagementPresence, ManagementProvenance, ManagementRelationKind, ManagementSource,
    };

    fn material(generation: u64, age_ms: u64) -> ManagementMaterial {
        let source = ManagementSource {
            generation: ManagementGeneration {
                endpoint: generation,
                observations: 0,
                route_expirations: 0,
            },
            observed_unix_ms: 1_000u64.saturating_sub(age_ms),
            provenance: ManagementProvenance::Announce,
            observation_sequence: Some(1),
        };
        let station = ManagementNodeId::from_source_key("destination:station");
        let peer = ManagementNodeId::from_source_key("destination:peer");
        ManagementMaterial {
            generation: source.generation,
            captured_unix_ms: 1_000,
            stale_after_ms: Duration::from_secs(60).as_millis() as u64,
            nodes: vec![
                ManagementNode {
                    id: station.clone(),
                    label: "Station".to_owned(),
                    roles: BTreeSet::from([ManagementRole::Station]),
                    announce_classes: BTreeSet::new(),
                    presence: ManagementPresence::Live,
                    source,
                },
                ManagementNode {
                    id: peer.clone(),
                    label: "Peer".to_owned(),
                    roles: BTreeSet::from([ManagementRole::Peer]),
                    announce_classes: BTreeSet::new(),
                    presence: if age_ms > Duration::from_secs(60).as_millis() as u64 {
                        ManagementPresence::Stale
                    } else {
                        ManagementPresence::Live
                    },
                    source,
                },
            ],
            relations: vec![ManagementRelation {
                id: ManagementRelationId::from_source_key("announce:1"),
                from: station,
                to: peer,
                kind: ManagementRelationKind::HeardAnnounce,
                label: "one hop".to_owned(),
                source,
            }],
        }
    }

    #[test]
    fn identical_source_is_a_true_noop_and_selection_survives_refresh() {
        let mut mere = DeviceMere::new();
        let first = material(1, 0);
        let receipt = mere.reconcile(&first);
        assert!(receipt.topology_changed);
        assert!(receipt.graph_edits > 0);
        let revision = receipt.revision;
        let epoch = receipt.topology_epoch;
        let selected = first.nodes[1].id.clone();
        assert!(mere.select(Some(selected.clone())));

        let same = mere.reconcile(&first);
        assert_eq!(same.graph_edits, 0);
        assert_eq!(same.revision, revision);
        assert_eq!(same.topology_epoch, epoch);
        assert_eq!(mere.selected(), Some(&selected));

        let mut refreshed = material(2, 5);
        refreshed.nodes[1].label = "Peer refreshed".to_owned();
        let refreshed_receipt = mere.reconcile(&refreshed);
        assert!(refreshed_receipt.graph_edits > 0);
        assert!(!refreshed_receipt.topology_changed);
        assert_eq!(refreshed_receipt.topology_epoch, epoch);
        assert_eq!(mere.selected(), Some(&selected));
    }

    #[test]
    fn projection_has_one_typed_relation_with_present_endpoints_and_retains_stale_nodes() {
        let mut mere = DeviceMere::new();
        let first = material(1, 0);
        let peer = first.nodes[1].id.clone();
        mere.reconcile(&first);
        let projection = mere.projection();
        assert_eq!(projection.relations.len(), 1);
        assert!(
            projection
                .nodes
                .iter()
                .any(|node| node.key == projection.relations[0].from)
        );
        assert!(
            projection
                .nodes
                .iter()
                .any(|node| node.key == projection.relations[0].to)
        );
        assert_eq!(
            projection.relations[0].fact.kind.vocabulary(),
            "signalman:heard-announce"
        );

        let mut disappeared = material(2, 0);
        disappeared.nodes.retain(|node| node.id != peer);
        disappeared.relations.clear();
        disappeared.captured_unix_ms = 62_000;
        mere.reconcile(&disappeared);
        let retained = mere
            .projection()
            .nodes
            .into_iter()
            .find(|node| node.fact.id == peer)
            .expect("disappeared source identity remains in the mere");
        assert_eq!(retained.fact.presence, ManagementPresence::Stale);
        assert!(retained.fact.roles.contains(&ManagementRole::KnownButStale));
    }

    #[test]
    fn absent_identity_waits_for_threshold_and_stales_only_once() {
        let mut mere = DeviceMere::new();
        let first = material(1, 0);
        let peer = first.nodes[1].id.clone();
        mere.reconcile(&first);

        let mut absent = material(2, 0);
        absent.nodes.retain(|node| node.id != peer);
        absent.relations.clear();
        absent.captured_unix_ms = 30_000;
        mere.reconcile(&absent);
        let before_threshold = mere
            .projection()
            .nodes
            .into_iter()
            .find(|node| node.fact.id == peer)
            .unwrap();
        assert_eq!(before_threshold.fact.presence, ManagementPresence::Live);
        let unchanged = mere.reconcile(&absent);
        assert_eq!(unchanged.graph_edits, 0);

        absent.captured_unix_ms = 62_000;
        let staled = mere.reconcile(&absent);
        assert_eq!(staled.graph_edits, 1);
        absent.captured_unix_ms = 120_000;
        let still_stale = mere.reconcile(&absent);
        assert_eq!(still_stale.graph_edits, 0);
    }
}
