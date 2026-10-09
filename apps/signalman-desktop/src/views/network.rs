//! The Network section: the management graph canvas and its companion rows.

use cambium::{GraphCanvasEvent, button, el, graph_canvas, text};

use crate::network::swatch_from_projection;
use crate::state::{DesktopState, LabelDensity};

use super::{Child, field, heading};

pub(super) fn network_page(state: &DesktopState) -> Child {
    // This is the one canonical projection for this view build. The canvas and
    // companion rows below consume it together, so accessibility cannot drift
    // into a second network assembled from similar-looking facts.
    let projection = state.network_projection();
    let swatch = swatch_from_projection(
        &projection,
        state.network_layout.as_ref(),
        state.device_mere.selected(),
        state.network_pan,
        state.network_zoom,
        state.management_settings.label_density == LabelDensity::Shown,
    );
    let nodes: Vec<Child> = projection
        .nodes
        .iter()
        .map(|node| {
            let id = node.fact.id.clone();
            let stale = node.fact.presence == signalman::management::ManagementPresence::Stale;
            let roles = node
                .fact
                .roles
                .iter()
                .map(|role| format!("{role:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            let label = if stale {
                format!("{}; stale; {roles}", node.fact.label)
            } else {
                format!("{}; live; {roles}", node.fact.label)
            };
            let selected = state.device_mere.selected() == Some(&node.fact.id);
            Box::new(
                button(label, move |s: &mut DesktopState, _| {
                    s.select_network_node(id.clone())
                })
                .attr(
                    "class",
                    if selected {
                        "network-row selected"
                    } else {
                        "network-row"
                    },
                )
                .attr("data-companion-key", node.fact.id.as_str().to_owned())
                .attr("aria-pressed", selected.to_string()),
            ) as Child
        })
        .collect();
    let relations: Vec<Child> = projection
        .relations
        .iter()
        .map(|relation| {
            let id = relation.id.as_str().to_owned();
            let selected = state.selected_relation.as_ref() == Some(&relation.id);
            Box::new(
                button(
                    format!(
                        "{}; {}",
                        relation.fact.label,
                        relation.fact.kind.vocabulary()
                    ),
                    move |s: &mut DesktopState, _| s.select_network_relation(&id),
                )
                .attr(
                    "class",
                    if selected {
                        "network-relation selected"
                    } else {
                        "network-relation"
                    },
                )
                .attr(
                    "data-companion-relation-id",
                    relation.id.as_str().to_owned(),
                )
                .attr("aria-pressed", selected.to_string()),
            ) as Child
        })
        .collect();
    let canvas: Child = Box::new(graph_canvas(
        &swatch,
        |s: &mut DesktopState, event| match event {
            GraphCanvasEvent::Activate(id) => s.select_network_node(id),
            GraphCanvasEvent::Drag(drag) => {
                s.drag_network_node(&drag.id, drag.phase, drag.position)
            }
            GraphCanvasEvent::RelationActivate(id) => s.select_network_relation(&id),
            GraphCanvasEvent::Expand => {}
            GraphCanvasEvent::Pan { delta } => s.pan_network(delta.0, delta.1),
            GraphCanvasEvent::Zoom { factor } => s.zoom_network(factor),
        },
    ));
    let empty: Child = if projection.nodes.is_empty() {
        Box::new(
            el(
                "div",
                text("No management snapshot is attached to this station yet."),
            )
            .attr("class", "empty")
            .attr("role", "status"),
        )
    } else {
        Box::new(el("div", ()).attr("class", "empty-none"))
    };
    let station: Child = if let Some(notice) = &state.station_notice {
        Box::new(
            el("div", text(notice.clone()))
                .attr("class", "station-status")
                .attr("role", "status"),
        )
    } else {
        Box::new(el("div", ()).attr("class", "station-status-none"))
    };
    let settings = &state.management_settings;
    let label_action = match settings.label_density {
        LabelDensity::Hidden => "Show node labels",
        LabelDensity::Shown => "Hide node labels",
    };
    let history_action = if settings.show_last_known {
        "Hide last-known devices"
    } else {
        "Show last-known devices"
    };

    Box::new(
        el(
            "main",
            (
                heading(
                    "Network",
                    "Observed management facts, retained as one local device graph.",
                ),
                el(
                    "section",
                    (
                        el("h2", text("Network settings")).attr("class", "network-heading"),
                        field(
                            "Stale age",
                            format!(
                                "{} minutes; used when management snapshots are projected",
                                settings.stale_age_minutes
                            ),
                        ),
                        el(
                            "div",
                            (
                                button("Shorter stale age", |s: &mut DesktopState, _| {
                                    s.shorten_stale_age()
                                }),
                                button("Longer stale age", |s: &mut DesktopState, _| {
                                    s.lengthen_stale_age()
                                }),
                            ),
                        )
                        .attr("class", "settings-controls"),
                        field(
                            "Announce history",
                            format!(
                                "{} observations; applies on the next station connection",
                                settings.announce_history_bound
                            ),
                        ),
                        el(
                            "div",
                            (
                                button("Keep less history", |s: &mut DesktopState, _| {
                                    s.reduce_history_bound()
                                }),
                                button("Keep more history", |s: &mut DesktopState, _| {
                                    s.increase_history_bound()
                                }),
                            ),
                        )
                        .attr("class", "settings-controls"),
                        field(
                            "Force strength",
                            format!(
                                "{:.0}% of the Seiche defaults",
                                settings.force_strength * 100.0
                            ),
                        ),
                        el(
                            "div",
                            (
                                button("Weaker layout forces", |s: &mut DesktopState, _| {
                                    s.reduce_force_strength()
                                }),
                                button("Stronger layout forces", |s: &mut DesktopState, _| {
                                    s.increase_force_strength()
                                }),
                            ),
                        )
                        .attr("class", "settings-controls"),
                        field("Layout damping", format!("{:.1}", settings.layout_damping)),
                        el(
                            "div",
                            (
                                button("Less damping", |s: &mut DesktopState, _| {
                                    s.reduce_layout_damping()
                                }),
                                button("More damping", |s: &mut DesktopState, _| {
                                    s.increase_layout_damping()
                                }),
                            ),
                        )
                        .attr("class", "settings-controls"),
                        el(
                            "div",
                            (
                                button(label_action, |s: &mut DesktopState, _| {
                                    s.toggle_network_labels()
                                }),
                                button(history_action, |s: &mut DesktopState, _| {
                                    s.toggle_last_known()
                                }),
                                button("Reset management settings", |s: &mut DesktopState, _| {
                                    s.reset_management_settings()
                                }),
                            ),
                        )
                        .attr("class", "settings-controls"),
                    ),
                )
                .attr("class", "management-settings")
                .attr("aria-label", "Network settings"),
                station,
                empty,
                // The viewport controls live on the canvas frame itself: the
                // viewport they move is this canvas, not a page setting.
                el(
                    "div",
                    (
                        el(
                            "div",
                            (
                                button("Pan left", |s: &mut DesktopState, _| {
                                    s.pan_network(-0.1, 0.0)
                                }),
                                button("Pan right", |s: &mut DesktopState, _| {
                                    s.pan_network(0.1, 0.0)
                                }),
                                button("Pan up", |s: &mut DesktopState, _| {
                                    s.pan_network(0.0, -0.1)
                                }),
                                button("Pan down", |s: &mut DesktopState, _| {
                                    s.pan_network(0.0, 0.1)
                                }),
                                button("Zoom in", |s: &mut DesktopState, _| s.zoom_network(1.2)),
                                button("Zoom out", |s: &mut DesktopState, _| {
                                    s.zoom_network(1.0 / 1.2)
                                }),
                                button("Reset view", |s: &mut DesktopState, _| {
                                    s.reset_network_view()
                                }),
                            ),
                        )
                        .attr("class", "network-controls")
                        .attr("aria-label", "Network viewport"),
                        canvas,
                    ),
                )
                .attr("class", "network-canvas"),
                el(
                    "section",
                    (
                        el("h2", text("Devices")).attr("class", "network-heading"),
                        el("div", nodes).attr("class", "network-rows"),
                    ),
                )
                .attr("aria-label", "Network devices"),
                el(
                    "section",
                    (
                        el("h2", text("Observed relations")).attr("class", "network-heading"),
                        el("div", relations).attr("class", "network-relations"),
                    ),
                )
                .attr("aria-label", "Network relations"),
            ),
        )
        .attr("class", "network-page")
        .attr("role", "main"),
    )
}
