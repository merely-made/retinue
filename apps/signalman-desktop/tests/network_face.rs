//! The management graph's shared canvas and semantic companion receipt.

use std::collections::BTreeSet;

use accesskit::Role;
use cambium_genet_winit_host::Harness;
use genet_probe::Selector;
use signalman::management::{
    AnnounceClassification, ManagementGeneration, ManagementMaterial, ManagementNode,
    ManagementNodeId, ManagementPresence, ManagementProvenance, ManagementRelation,
    ManagementRelationId, ManagementRelationKind, ManagementRole, ManagementSource,
};
use signalman_desktop::state::{DesktopSection, DesktopState, NetworkRequest};
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::{default_catalog_path, root, sheet};

type App = Harness<DesktopState, Logic, Child>;

fn source(observation_sequence: Option<u64>) -> ManagementSource {
    ManagementSource {
        generation: ManagementGeneration {
            endpoint: 4,
            observations: 7,
            route_expirations: 0,
        },
        observed_unix_ms: 1_000,
        provenance: ManagementProvenance::Announce,
        observation_sequence,
    }
}

fn material() -> ManagementMaterial {
    let station = ManagementNodeId::from_source_key("destination:station");
    let peer = ManagementNodeId::from_source_key("destination:peer");
    let unknown = ManagementNodeId::from_source_key("destination:unknown");
    let generation = source(None).generation;
    ManagementMaterial {
        generation,
        captured_unix_ms: 2_000,
        stale_after_ms: 60_000,
        nodes: vec![
            ManagementNode {
                id: station.clone(),
                label: "Signalman station".into(),
                roles: BTreeSet::from([ManagementRole::Station]),
                announce_classes: BTreeSet::new(),
                presence: ManagementPresence::Live,
                source: source(None),
            },
            ManagementNode {
                id: peer.clone(),
                label: "Workshop peer".into(),
                roles: BTreeSet::from([ManagementRole::Peer]),
                announce_classes: BTreeSet::from([AnnounceClassification::Delivery]),
                presence: ManagementPresence::Live,
                source: source(Some(3)),
            },
            ManagementNode {
                id: unknown.clone(),
                label: "Unknown destination".into(),
                roles: BTreeSet::from([ManagementRole::KnownButStale]),
                announce_classes: BTreeSet::from([AnnounceClassification::Unknown]),
                presence: ManagementPresence::Stale,
                source: source(Some(4)),
            },
        ],
        relations: vec![
            ManagementRelation {
                id: ManagementRelationId::from_source_key("announce:peer"),
                from: station.clone(),
                to: peer,
                kind: ManagementRelationKind::HeardAnnounce,
                label: "one hop on local interface".into(),
                source: source(Some(3)),
            },
            ManagementRelation {
                id: ManagementRelationId::from_source_key("route:unknown"),
                from: unknown,
                to: station,
                kind: ManagementRelationKind::RouteVia,
                label: "routed via station".into(),
                source: source(None),
            },
        ],
    }
}

fn harness() -> App {
    let mut state = DesktopState::new(&default_catalog_path());
    state.apply_management_material(&material());
    state.show_section(DesktopSection::Network);
    let mut harness = Harness::new(sheet(), state, root as Logic);
    harness.layout_at(1100.0, 900.0);
    harness
}

#[test]
fn canvas_and_companion_expose_the_same_stable_id_sets() {
    let mut harness = harness();
    let projection = harness.state().network_projection();
    for node in &projection.nodes {
        let id = node.fact.id.as_str();
        assert!(
            harness
                .resolve(&Selector::role("button").with_attr("data-key", id))
                .is_some(),
            "canvas exposes node {id}"
        );
        assert!(
            harness
                .resolve(&Selector::role("button").with_attr("data-companion-key", id))
                .is_some(),
            "companion exposes node {id}"
        );
        assert!(harness.click_on(&Selector::role("button").with_attr("data-key", id)));
        assert_eq!(harness.state().device_mere.selected(), Some(&node.fact.id));
    }
    for relation in &projection.relations {
        let id = relation.id.as_str();
        assert!(
            harness
                .resolve(&Selector::role("button").with_attr("data-relation-id", id))
                .is_some(),
            "canvas exposes relation {id}"
        );
        assert!(
            harness
                .resolve(&Selector::role("button").with_attr("data-companion-relation-id", id))
                .is_some(),
            "companion exposes relation {id}"
        );
    }

    let (tree, _) = harness.a11y_tree();
    assert!(tree.nodes.iter().any(|(_, node)| {
        node.role() == Role::Button
            && node
                .label()
                .is_some_and(|label| label.contains("Unknown destination; stale"))
    }));
}

#[test]
fn selection_drag_and_named_viewport_controls_use_the_shipping_handlers() {
    let mut harness = harness();
    let selected = ManagementNodeId::from_source_key("destination:peer");
    let selector = Selector::role("button").with_attr("data-key", selected.as_str());
    let point = harness
        .resolve(&selector)
        .expect("canvas target has a rect");
    harness.press_at(point.0, point.1);
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Pin(_, _))
    ));
    harness.move_to(point.0 + 24.0, point.1 + 12.0);
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Pin(_, _))
    ));
    harness.release_at(point.0 + 24.0, point.1 + 12.0);
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Unpin(_))
    ));
    assert_eq!(harness.state().device_mere.selected(), Some(&selected));

    let pan = harness.state().network_pan;
    assert!(harness.click_on(&Selector::role("button").containing("Pan right")));
    assert!(harness.state().network_pan.0 > pan.0);
    let zoom = harness.state().network_zoom;
    assert!(harness.click_on(&Selector::role("button").containing("Zoom in")));
    assert!(harness.state().network_zoom > zoom);
    assert_eq!(harness.state().device_mere.selected(), Some(&selected));
}

#[test]
fn a_dragged_node_tracks_the_pointer_instead_of_a_stale_actor_snapshot() {
    let mut harness = harness();
    let dragged = ManagementNodeId::from_source_key("destination:peer");
    let key = harness
        .state()
        .network_projection()
        .nodes
        .iter()
        .find(|node| node.fact.id == dragged)
        .map(|node| node.key)
        .expect("dragged node is in the projection");

    // Seed a settled layout, then drag: the painted position must echo the
    // pointer immediately, without waiting for the actor round trip.
    let seeded = signalman_desktop::network::NetworkLayout {
        epoch: harness.state().network_epoch,
        snapshot: seiche::LayoutSnapshot {
            positions: vec![(key, euclid::default::Point2D::new(0.0, 0.0))],
            ..Default::default()
        },
        worker_thread: std::thread::current().id(),
    };
    let mut adopted = false;
    let seed_again = seeded.clone();
    harness.update(|state| adopted = state.adopt_network_layout(seeded));
    assert!(adopted);
    harness.update(|state| {
        state.drag_network_node(&dragged, cambium::PointerPhase::Down, (0.25, 0.25));
        state.drag_network_node(&dragged, cambium::PointerPhase::Move, (0.75, 0.75));
    });
    let echoed = layout_position(harness.state(), key);
    assert!(
        echoed.x > 0.0 && echoed.y > 0.0,
        "paint echoes the pointer, got {echoed:?}"
    );

    // A stale actor snapshot arriving mid-drag must not flick the dragged
    // node back; the other bodies stay physics-authoritative.
    harness.update(|state| adopted = state.adopt_network_layout(seed_again));
    assert!(adopted);
    assert_eq!(layout_position(harness.state(), key), echoed);

    // Releasing hands the node back to physics.
    harness.update(|state| {
        state.drag_network_node(&dragged, cambium::PointerPhase::Up, (0.75, 0.75));
    });
    let settled = signalman_desktop::network::NetworkLayout {
        epoch: harness.state().network_epoch,
        snapshot: seiche::LayoutSnapshot {
            positions: vec![(key, euclid::default::Point2D::new(5.0, 5.0))],
            ..Default::default()
        },
        worker_thread: std::thread::current().id(),
    };
    harness.update(|state| adopted = state.adopt_network_layout(settled));
    assert!(adopted);
    assert_eq!(
        layout_position(harness.state(), key),
        euclid::default::Point2D::new(5.0, 5.0)
    );
}

fn layout_position(
    state: &signalman_desktop::state::DesktopState,
    key: seiche::NodeKey,
) -> euclid::default::Point2D<f32> {
    state
        .network_layout
        .as_ref()
        .and_then(|layout| {
            layout
                .positions
                .iter()
                .find(|(existing, _)| *existing == key)
                .map(|(_, position)| *position)
        })
        .expect("layout retains the dragged node")
}

#[test]
fn last_known_visibility_filters_the_shared_view_without_erasing_retained_history() {
    let mut harness = harness();
    assert_eq!(harness.state().device_mere.projection().nodes.len(), 3);
    assert_eq!(harness.state().device_mere.projection().relations.len(), 2);

    assert!(harness.click_on(&Selector::role("button").containing("Hide last-known devices")));
    let visible = harness.state().network_projection();
    assert_eq!(visible.nodes.len(), 2);
    assert_eq!(visible.relations.len(), 1);
    assert!(
        visible
            .nodes
            .iter()
            .all(|node| node.fact.presence == ManagementPresence::Live)
    );
    assert_eq!(
        harness.state().device_mere.projection().nodes.len(),
        3,
        "the owner hid retained history from this view; Chartulary did not forget it"
    );

    assert!(harness.click_on(&Selector::role("button").containing("Show last-known devices")));
    assert_eq!(harness.state().network_projection().nodes.len(), 3);
    assert_eq!(harness.state().network_projection().relations.len(), 2);
}

#[test]
fn hidden_last_known_refreshes_reconcile_visible_topology_and_selection() {
    let mut harness = harness();
    assert!(harness.click_on(&Selector::role("button").containing("Hide last-known devices")));
    harness.update(|state| {
        let _ = state.take_network_request();
    });
    let starting_epoch = harness.state().network_epoch;
    let unknown = ManagementNodeId::from_source_key("destination:unknown");

    let mut live = material();
    let live_unknown = live
        .nodes
        .iter_mut()
        .find(|node| node.id == unknown)
        .expect("fixture has the unknown destination");
    live_unknown.presence = ManagementPresence::Live;
    live_unknown.roles.remove(&ManagementRole::KnownButStale);
    harness.update(move |state| {
        let _ = state.apply_management_material(&live);
    });
    assert_eq!(harness.state().network_projection().nodes.len(), 3);
    assert!(harness.state().network_epoch > starting_epoch);
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Reconcile(_))
    ));

    harness.update(|state| {
        let _ = state.take_network_request();
        state.select_network_node(unknown.clone());
    });
    let live_epoch = harness.state().network_epoch;
    let stale = material();
    harness.update(move |state| {
        let _ = state.apply_management_material(&stale);
    });
    assert_eq!(harness.state().network_projection().nodes.len(), 2);
    assert!(harness.state().device_mere.selected().is_none());
    assert!(harness.state().network_epoch > live_epoch);
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Reconcile(_))
    ));
}
