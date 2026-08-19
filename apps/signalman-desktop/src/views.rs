//! The six owner pages.
//!
//! Everything a page shows comes from the flow's own projection. The review
//! page in particular renders `FirmwareReview` field by field: package identity
//! and version, publisher, every artifact hash and address, license and source, origin, board
//! revision and its evidence, route and helper provenance, write and preserved ranges, state
//! impact, and recovery instructions. None of it is summarized away, because
//! the whole point of a review page is that the owner can check it.
//!
//! Refusals and warnings are visible states. There is no disabled control with
//! a missing explanation anywhere in this file: a step that cannot proceed says
//! why, in the words Linkboy structured.

use cambium::{AnyView, GenetCtx, GenetElement, GraphCanvasEvent, button, el, graph_canvas, text};
use linkboy::{OwnerStage, ReceiptResult, StateImpact};
use signalman::message::{MessageDirection, MessageId};

use crate::network::swatch_from_projection;
use crate::state::{
    DesktopSection, DesktopState, LabelDensity, MESHNOLOGY_N39_DOCUMENTATION_URL, Request,
    SurveyState, VOICE_DURATION_OPTIONS, VOICE_ENCODING_OPTIONS, VoiceActivity,
};

pub type Child = Box<dyn AnyView<DesktopState, (), GenetCtx, GenetElement>>;
pub type Logic = fn(&DesktopState) -> Child;

/// A labelled row of static text — the review page's unit.
fn field(label: &str, value: impl Into<String>) -> Child {
    Box::new(
        el(
            "div",
            (
                el("div", text(label.to_string())).attr("class", "field-label"),
                el("div", text(value.into())).attr("class", "field-value"),
            ),
        )
        .attr("class", "field"),
    )
}

fn heading(title: &str, subtitle: &str) -> Child {
    Box::new(
        el(
            "div",
            (
                el("h1", text(title.to_string())).attr("class", "page-title"),
                el("div", text(subtitle.to_string())).attr("class", "page-subtitle"),
            ),
        )
        .attr("class", "page-head"),
    )
}

/// The refusal panel. Present whenever there is something to say, absent
/// otherwise — never a disabled button with no explanation.
fn refusal(state: &DesktopState) -> Child {
    if state.refusal.is_empty() {
        return Box::new(el("div", ()).attr("class", "refusal-empty"));
    }
    let lines: Vec<Child> = state
        .refusal
        .iter()
        .map(|line| -> Child {
            Box::new(el("li", text(line.clone())).attr("class", "refusal-line"))
        })
        .collect();
    Box::new(
        el(
            "div",
            (
                el("div", text("This cannot go ahead yet")).attr("class", "refusal-title"),
                el("ul", lines).attr("class", "refusal-list"),
            ),
        )
        .attr("class", "refusal")
        .attr("role", "alert"),
    )
}

/// The six-step trail, so an owner always knows where they are. Read-only: the
/// flow owns page transitions, and a stepper that let you jump would be a
/// second flow.
fn trail(stage: OwnerStage) -> Child {
    let steps = [
        (OwnerStage::ChooseDevice, "Choose device"),
        (OwnerStage::ChooseFirmware, "Choose firmware"),
        (OwnerStage::ReviewChanges, "Review changes"),
        (OwnerStage::PrepareDevice, "Prepare device"),
        (OwnerStage::Install, "Install"),
        (OwnerStage::VerifyOrRecover, "Verify or recover"),
    ];
    let here = steps.iter().position(|(s, _)| *s == stage).unwrap_or(0);
    let items: Vec<Child> = steps
        .iter()
        .enumerate()
        .map(|(i, (_, label))| -> Child {
            let class = match i.cmp(&here) {
                std::cmp::Ordering::Less => "trail-step done",
                std::cmp::Ordering::Equal => "trail-step here",
                std::cmp::Ordering::Greater => "trail-step ahead",
            };
            // The `<ol>` supplies the number; repeating it here would read
            // "1. 1. Choose device" to eye and screen reader alike.
            Box::new(
                el("li", text((*label).to_string()))
                    .attr("class", class)
                    .attr("aria-current", if i == here { "step" } else { "false" }),
            )
        })
        .collect();
    Box::new(
        el("ol", items)
            .attr("class", "trail")
            .attr("aria-label", "Owner flow"),
    )
}

fn section_tab(label: &'static str, section: DesktopSection, selected: bool) -> Child {
    Box::new(
        button(label, move |state: &mut DesktopState, _| {
            state.show_section(section)
        })
        .attr(
            "class",
            if selected {
                "section-tab selected"
            } else {
                "section-tab"
            },
        )
        .attr("aria-pressed", selected.to_string())
        .attr("aria-current", if selected { "page" } else { "false" }),
    )
}

/// The application root: five stable sections and the selected face.
pub fn root(state: &DesktopState) -> Child {
    let tabs: Vec<Child> = [
        ("Devices", DesktopSection::Devices),
        ("Network", DesktopSection::Network),
        ("Messages", DesktopSection::Messages),
        ("Map", DesktopSection::Map),
        ("Browse", DesktopSection::Browse),
    ]
    .into_iter()
    .map(|(label, section)| section_tab(label, section, state.section == section))
    .collect();
    Box::new(
        el(
            "div",
            (
                el("nav", tabs)
                    .attr("class", "section-tabs")
                    .attr("aria-label", "Signalman sections"),
                match state.section {
                    DesktopSection::Devices => devices_face(state),
                    DesktopSection::Network => network_page(state),
                    DesktopSection::Messages => messages_page(state),
                    DesktopSection::Map => unavailable_page(
                        "Map",
                        "Map is unavailable until owner placement records land.",
                    ),
                    DesktopSection::Browse => unavailable_page(
                        "Browse",
                        "Browse is unavailable until document composition and source posture land.",
                    ),
                },
            ),
        )
        .attr("class", "app-shell"),
    )
}

fn unavailable_page(title: &'static str, gate: &'static str) -> Child {
    Box::new(
        el(
            "main",
            (
                heading(
                    title,
                    "This section has no synthetic data or placeholder actions.",
                ),
                el("div", text(gate)).attr("class", "unavailable-gate"),
            ),
        )
        .attr("class", "unavailable-page")
        .attr("role", "main")
        .attr("aria-label", title),
    )
}

fn messages_page(state: &DesktopState) -> Child {
    let rows = state
        .message_store
        .records()
        .rev()
        .map(|record| {
            let id = record.message.id();
            let peer = match record.direction {
                MessageDirection::Incoming => record.message.sender(),
                MessageDirection::Outgoing => record.message.recipient(),
            };
            let name = state
                .message_store
                .contact_name(peer)
                .map(str::to_owned)
                .unwrap_or_else(|| short_address(peer.destination));
            let direction = match record.direction {
                MessageDirection::Incoming => "From",
                MessageDirection::Outgoing => "To",
            };
            let content = record.message.text().map(str::to_owned).unwrap_or_else(|| {
                let facts = record.message.voice().unwrap().facts();
                format!(
                    "Voice drop, {}, {} ms, {} bytes",
                    facts.encoding.label(),
                    facts.duration_ms,
                    facts.encoded_bytes
                )
            });
            let mut label = format!("{direction} {name}: {}. {}", content, record.status.label());
            if let signalman::message::MessageStatus::Failed(reason) = &record.status {
                label.push_str(&format!(": {reason}"));
            }
            Box::new(
                button(label, move |s: &mut DesktopState, _| s.select_message(id))
                    .attr(
                        "class",
                        if state.selected_message == Some(id) {
                            "message-row selected"
                        } else {
                            "message-row"
                        },
                    )
                    .attr("data-message-id", message_id_hex(id)),
            ) as Child
        })
        .collect::<Vec<_>>();

    let history: Child = if rows.is_empty() {
        Box::new(el("div", text("There are no persisted messages yet.")).attr("class", "empty"))
    } else {
        Box::new(
            el("div", rows)
                .attr("class", "message-rows")
                .attr("role", "list"),
        )
    };

    let contact_controls: Child = state
        .selected_message
        .and_then(|id| {
            state
                .message_store
                .records()
                .find(|record| record.message.id() == id)
        })
        .filter(|record| record.direction == MessageDirection::Incoming)
        .map(|record| record.message.sender())
        .filter(|peer| peer.identity.is_some() && state.message_store.contact_name(*peer).is_none())
        .map(|_| {
            Box::new(
                el(
                    "div",
                    (
                        el("div", text("Save this authenticated sender"))
                            .attr("class", "network-heading"),
                        el(
                            "label",
                            (
                                el("div", text("Your name for them")).attr("class", "field-label"),
                                el(
                                    "div",
                                    cambium::lens(
                                        |input: &mut cambium::TextInput| cambium::text_field(input),
                                        |s: &mut DesktopState| &mut s.message_contact_name,
                                    ),
                                )
                                .attr("class", "revision-wrap")
                                .attr("data-text-field", "message-contact-name"),
                            ),
                        )
                        .attr("class", "revision-label"),
                        button("Save sender as contact", |s: &mut DesktopState, _| {
                            s.save_selected_sender()
                        })
                        .attr("class", "secondary"),
                    ),
                )
                .attr("class", "message-contact"),
            ) as Child
        })
        .unwrap_or_else(|| Box::new(el("div", ()).attr("class", "empty-none")));

    let notice: Child = state
        .message_notice
        .as_ref()
        .map(|notice| {
            Box::new(
                el("div", text(notice.clone()))
                    .attr("class", "message-notice")
                    .attr("role", "status"),
            ) as Child
        })
        .unwrap_or_else(|| Box::new(el("div", ()).attr("class", "empty-none")));

    let input_names = state
        .voice_inputs
        .iter()
        .map(|device| {
            if device.is_default {
                format!("{} (system default)", device.label)
            } else {
                device.label.clone()
            }
        })
        .collect::<Vec<_>>();
    let input_control: Child = if input_names.is_empty() {
        Box::new(el("div", text("No voice input device is available.")).attr("class", "hint"))
    } else {
        Box::new(
            el(
                "label",
                (
                    el("div", text("Input device")).attr("class", "field-label"),
                    cambium::lens(
                        move |choice: &mut cambium::SelectState| {
                            let options =
                                input_names.iter().map(String::as_str).collect::<Vec<_>>();
                            cambium::select(choice, &options)
                        },
                        |s: &mut DesktopState| &mut s.voice_input,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
        )
    };

    let output_names = state
        .voice_outputs
        .iter()
        .map(|device| {
            if device.is_default {
                format!("{} (system default)", device.label)
            } else {
                device.label.clone()
            }
        })
        .collect::<Vec<_>>();
    let output_control: Child = if output_names.is_empty() {
        Box::new(el("div", text("No voice output device is available.")).attr("class", "hint"))
    } else {
        Box::new(
            el(
                "label",
                (
                    el("div", text("Output device")).attr("class", "field-label"),
                    cambium::lens(
                        move |choice: &mut cambium::SelectState| {
                            let options =
                                output_names.iter().map(String::as_str).collect::<Vec<_>>();
                            cambium::select(choice, &options)
                        },
                        |s: &mut DesktopState| &mut s.voice_output,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
        )
    };

    let capture_control: Child = match state.voice_activity {
        VoiceActivity::Idle => Box::new(
            button("Record voice drop", |s: &mut DesktopState, _| {
                s.start_voice_capture()
            })
            .attr("class", "primary")
            .attr("data-voice-action", "record"),
        ),
        VoiceActivity::Recording => Box::new(
            button("Stop and queue voice drop", |s: &mut DesktopState, _| {
                s.stop_voice_capture()
            })
            .attr("class", "primary")
            .attr("data-voice-action", "stop"),
        ),
        _ => Box::new(
            el(
                "div",
                text(format!("Host audio: {}.", state.voice_activity.label())),
            )
            .attr("class", "voice-activity")
            .attr("role", "status"),
        ),
    };

    let selected_is_voice = state.selected_message.is_some_and(|id| {
        state
            .message_store
            .records()
            .find(|record| record.message.id() == id)
            .is_some_and(|record| record.message.voice().is_some())
    });
    let playback_control: Child = if selected_is_voice {
        Box::new(
            button("Play selected voice drop", |s: &mut DesktopState, _| {
                s.play_selected_voice()
            })
            .attr("class", "secondary")
            .attr("data-voice-action", "play"),
        )
    } else {
        Box::new(
            el("div", text("Select a voice drop in history to play it.")).attr("class", "hint"),
        )
    };

    let playback_receipt: Child = state
        .voice_playback_receipt
        .as_ref()
        .map(|receipt| {
            Box::new(
                el(
                    "div",
                    text(format!(
                        "Playback receipt: {} ms, {} Hz, {} channel{} through {}.",
                        receipt.decoded_duration_ms,
                        receipt.output_sample_rate,
                        receipt.output_channels,
                        if receipt.output_channels == 1 {
                            ""
                        } else {
                            "s"
                        },
                        receipt.device_label,
                    )),
                )
                .attr("class", "voice-receipt"),
            ) as Child
        })
        .unwrap_or_else(|| Box::new(el("div", ()).attr("class", "empty-none")));

    let voice_controls = el(
        "div",
        (
            el("div", text("Voice drop")).attr("class", "network-heading"),
            el(
                "div",
                text("Recording is downmixed to 8 kHz mono, encoded once, and persisted before transport."),
            )
            .attr("class", "hint"),
            input_control,
            output_control,
            el(
                "label",
                (
                    el("div", text("Encoding")).attr("class", "field-label"),
                    cambium::lens(
                        |choice: &mut cambium::SelectState| {
                            cambium::select(choice, &VOICE_ENCODING_OPTIONS)
                        },
                        |s: &mut DesktopState| &mut s.voice_encoding,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
            el(
                "label",
                (
                    el("div", text("Maximum duration")).attr("class", "field-label"),
                    cambium::lens(
                        |choice: &mut cambium::SelectState| {
                            cambium::select(choice, &VOICE_DURATION_OPTIONS)
                        },
                        |s: &mut DesktopState| &mut s.voice_duration,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
            capture_control,
            playback_control,
            playback_receipt,
        ),
    )
    .attr("class", "voice-compose");

    Box::new(
        el(
            "main",
            (
                heading(
                    "Messages",
                    "Conversation history is replayed from the local Codicil log.",
                ),
                el(
                    "div",
                    (
                        el(
                            "label",
                            (
                                el("div", text("Recipient address"))
                                    .attr("class", "field-label"),
                                el(
                                    "div",
                                    cambium::lens(
                                        |input: &mut cambium::TextInput| cambium::text_field(input),
                                        |s: &mut DesktopState| &mut s.message_recipient,
                                    ),
                                )
                                .attr("class", "revision-wrap")
                                .attr("data-text-field", "message-recipient"),
                            ),
                        )
                        .attr("class", "revision-label"),
                        el(
                            "label",
                            (
                                el("div", text("Message"))
                                    .attr("class", "field-label"),
                                el(
                                    "div",
                                    cambium::lens(
                                        |input: &mut cambium::TextInput| cambium::text_field(input),
                                        |s: &mut DesktopState| &mut s.message_draft,
                                    ),
                                )
                                .attr("class", "revision-wrap")
                                .attr("data-text-field", "message-draft"),
                            ),
                        )
                        .attr("class", "revision-label"),
                        button("Queue message", |s: &mut DesktopState, _| s.queue_message())
                            .attr("class", "primary"),
                        el(
                            "div",
                            text(if state.message_local.is_some() {
                                "Outgoing intent is persisted before transport is attempted."
                            } else {
                                "A station identity is not connected. Drafts cannot be queued under an invented sender."
                            }),
                        )
                        .attr("class", "hint"),
                        notice,
                    ),
                )
                .attr("class", "message-compose"),
                voice_controls,
                el("div", text("Conversation history")).attr("class", "network-heading"),
                history,
                contact_controls,
            ),
        )
        .attr("class", "messages-page")
        .attr("role", "main")
        .attr("aria-label", "Messages"),
    )
}

fn short_address(bytes: [u8; 16]) -> String {
    bytes[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn message_id_hex(id: MessageId) -> String {
    id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn devices_face(state: &DesktopState) -> Child {
    let stage = state.stage();
    Box::new(
        el(
            "div",
            (
                trail(stage),
                el(
                    "main",
                    (
                        match stage {
                            OwnerStage::ChooseDevice => choose_device(state),
                            OwnerStage::ChooseFirmware => choose_firmware(state),
                            OwnerStage::ReviewChanges => review_changes(state),
                            OwnerStage::PrepareDevice => prepare_device(state),
                            OwnerStage::Install => install(state),
                            OwnerStage::VerifyOrRecover => verify_or_recover(state),
                        },
                        refusal(state),
                    ),
                )
                .attr("class", "page")
                .attr("role", "main"),
            ),
        )
        .attr("class", "shell"),
    )
}

fn network_page(state: &DesktopState) -> Child {
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
                    "div",
                    (
                        button("Pan left", |s: &mut DesktopState, _| {
                            s.pan_network(-0.1, 0.0)
                        }),
                        button("Pan right", |s: &mut DesktopState, _| {
                            s.pan_network(0.1, 0.0)
                        }),
                        button("Pan up", |s: &mut DesktopState, _| s.pan_network(0.0, -0.1)),
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
                empty,
                el("div", canvas).attr("class", "network-canvas"),
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

// ------------------------------------------------------------ 1. device

fn choose_device(state: &DesktopState) -> Child {
    let rows: Vec<Child> = state
        .devices
        .iter()
        .enumerate()
        .map(|(index, device)| -> Child {
            let selected = state.selected_device == Some(index);
            Box::new(
                button(device.summary(), move |s: &mut DesktopState, _| {
                    s.select_device(index)
                })
                .attr("class", if selected { "row selected" } else { "row" })
                .attr("aria-pressed", if selected { "true" } else { "false" })
                .attr("data-port", device.port.clone()),
            )
        })
        .collect();
    let empty: Child = match state.survey {
        SurveyState::Unasked => {
            Box::new(el("div", text("Looking for boards…")).attr("class", "empty"))
        }
        SurveyState::Surveyed if state.devices.is_empty() => Box::new(
            el(
                "div",
                text(
                    "No serial ports. Plug the board in with a data cable — a \
                     charge-only cable enumerates nothing.",
                ),
            )
            .attr("class", "empty"),
        ),
        SurveyState::Surveyed => Box::new(el("div", ()).attr("class", "empty-none")),
    };
    // A recognized board can offer a package-compatible revision as an explicit choice.
    // It is deliberately not a default: the carrier's printing, not the USB banner, is the
    // authority for this claim. A silent foreign T114 needs the owner's explicit family
    // declaration plus its captured loader record.
    let family = state
        .device()
        .and_then(|device| match device.board.as_deref() {
            Some("HeltecV4") => Some(linkboy::BoardFamily::HeltecV4),
            Some("T114") => Some(linkboy::BoardFamily::T114),
            _ => None,
        })
        .or_else(|| state.selected_board_family.clone());
    let is_t114 = matches!(&family, Some(linkboy::BoardFamily::T114));
    let selected_device_is_silent = state.device().is_some_and(|device| device.board.is_none());
    let known_revision: Child = match family {
        Some(linkboy::BoardFamily::HeltecV4) => Box::new(el(
            "div",
            (
                button("Use V4 revision 4.2", |s: &mut DesktopState, _| {
                    s.select_board_revision("4.2")
                })
                .attr("class", "secondary")
                .attr(
                    "aria-description",
                    "Select only when 4.2 is printed on the Heltec V4 board.",
                ),
                button(
                    "Use Meshnology N39 V4.2 profile",
                    |s: &mut DesktopState, _| s.select_meshnology_n39_v4_2_profile(),
                )
                .attr("class", "secondary")
                .attr(
                    "aria-description",
                    format!(
                        "Select only for the Meshnology N39 kit. Its published product documentation names the V4.2 schematic: {MESHNOLOGY_N39_DOCUMENTATION_URL}"
                    ),
                ),
            ),
        )),
        Some(linkboy::BoardFamily::T114) => Box::new(
            button("Use T114 revision 2.x", |s: &mut DesktopState, _| {
                s.select_board_revision("2.x")
            })
            .attr("class", "secondary")
            .attr(
                "aria-description",
                "Select only when the T114 matches the package's 2.x profile.",
            ),
        ),
        _ => Box::new(el("div", ()).attr("class", "empty-none")),
    };
    // An owner declaration is an escape hatch for a silent serial port. A
    // board that named itself has supplied the stronger fact already, so the
    // declarations are neither useful nor keyboard stops on its chooser page.
    // Selecting a family only permits the corresponding non-writing evidence
    // path; it does not turn the COM location into hardware evidence.
    let declare_silent_device: Child = if selected_device_is_silent {
        Box::new(
            el(
                "div",
                (
                    button("This serial device is a V4", |s: &mut DesktopState, _| {
                        s.select_board_family(linkboy::BoardFamily::HeltecV4)
                    })
                    .attr("class", "secondary")
                    .attr(
                        "aria-description",
                        "Declare the selected silent serial device to be the V4 you own. Linkboy will still inspect its ESP ROM loader before planning.",
                    ),
                    button("This serial device is a T114", |s: &mut DesktopState, _| {
                        s.select_board_family(linkboy::BoardFamily::T114)
                    })
                    .attr("class", "secondary")
                    .attr(
                        "aria-description",
                        "Declare the selected silent serial device to be the T114 you own. A retained UF2 loader record is still required.",
                    ),
                ),
            )
            .attr("class", "actions"),
        )
    } else {
        Box::new(el("div", ()).attr("class", "empty-none"))
    };
    let t114_dfu_recovery: Child = if is_t114 && selected_device_is_silent {
        Box::new(
            el(
                "div",
                (
                    el("div", text("T114 DFU recovery")).attr("class", "field-label"),
                    el(
                        "div",
                        text(
                            "Use this only after the selected silent port is already in the T114 serial-DFU loader. Linkboy will use the retained loader record and will not ask an absent application to enter DFU again.",
                        ),
                    )
                    .attr("class", "hint"),
                    button("Use selected T114 DFU port", |s: &mut DesktopState, _| {
                        s.request(Request::ConfirmT114Dfu)
                    })
                    .attr("class", "secondary")
                    .attr(
                        "aria-description",
                        "Confirm that the selected silent port is already the DFU loader captured in the retained T114 loader record.",
                    ),
                ),
            )
            .attr("class", "revision-row"),
        )
    } else {
        Box::new(el("div", ()).attr("class", "empty-none"))
    };
    let t114_uf2_route: Child = if is_t114 {
        Box::new(
            el(
                "div",
                (
                    el("div", text("T114 UF2 route")).attr("class", "field-label"),
                    el(
                        "label",
                        (
                            el("div", text("Mounted UF2 volume")).attr("class", "field-label"),
                            el(
                                "div",
                                cambium::lens(
                                    |input: &mut cambium::TextInput| cambium::text_field(input),
                                    |s: &mut DesktopState| &mut s.t114_uf2_volume,
                                ),
                            )
                            .attr("class", "revision-wrap")
                            .attr("data-text-field", "uf2-volume"),
                        ),
                    )
                    .attr("class", "revision-label"),
                    el(
                        "label",
                        (
                            el("div", text("Loader record path")).attr("class", "field-label"),
                            el(
                                "div",
                                cambium::lens(
                                    |input: &mut cambium::TextInput| cambium::text_field(input),
                                    |s: &mut DesktopState| &mut s.t114_loader_record,
                                ),
                            )
                            .attr("class", "revision-wrap")
                            .attr("data-text-field", "loader-record"),
                        ),
                    )
                    .attr("class", "revision-label"),
                    el(
                        "div",
                        text(
                            "For an upstream T114 UF2 install, Linkboy reads this mounted volume and saves its own loader and SoftDevice record here for the later serial restore.",
                        ),
                    )
                    .attr("class", "hint"),
                    button("Use mounted T114 volume", |s: &mut DesktopState, _| {
                        s.request(Request::ConfirmMountedT114)
                    })
                    .attr("class", "secondary"),
                ),
            )
            .attr("class", "revision-row"),
        )
    } else {
        Box::new(el("div", ()).attr("class", "empty-none"))
    };
    Box::new(el(
        "div",
        (
            heading(
                "Choose device",
                "Every port this machine has, and what answered on it.",
            ),
            el("div", rows).attr("class", "rows").attr("role", "list"),
            empty,
            el(
                "div",
                (
                    // The `<label>` *wraps* the field rather than pointing at
                    // it: `text_field` generates a bare `<input>` with no id, so
                    // `for` would name nothing and a screen reader would say
                    // "edit, blank". Wrapping needs no id and is how HTML has
                    // always named a generated control.
                    el(
                        "label",
                        (
                            el("div", text("Board revision")).attr("class", "field-label"),
                            // A real editable field: the host's caret,
                            // selection, IME, and visual movement all run
                            // against it through the `focused_text` seam.
                            el(
                                "div",
                                cambium::lens(
                                    |input: &mut cambium::TextInput| cambium::text_field(input),
                                    |s: &mut DesktopState| &mut s.board_revision,
                                ),
                            )
                            .attr("class", "revision-wrap")
                            .attr("data-text-field", "revision"),
                        ),
                    )
                    .attr("class", "revision-label"),
                    el(
                        "div",
                        text(
                            "As printed on the board, or from a named documented product \
                             profile. Nothing on the wire identifies a revision, so Linkboy \
                             records the source before it plans a flash.",
                        ),
                    )
                    .attr("class", "hint"),
                    known_revision,
                ),
            )
            .attr("class", "revision-row"),
            declare_silent_device,
            t114_uf2_route,
            t114_dfu_recovery,
            el(
                "div",
                (
                    button("Rescan", |s: &mut DesktopState, _| {
                        s.request(Request::Rescan)
                    })
                    .attr("class", "secondary"),
                    button("Use this device", |s: &mut DesktopState, _| {
                        s.request(Request::ConfirmDevice)
                    })
                    .attr("class", "primary"),
                ),
            )
            .attr("class", "actions"),
        ),
    ))
}

// ---------------------------------------------------------- 2. firmware

fn choose_firmware(state: &DesktopState) -> Child {
    let catalog_note: Child = match (&state.catalog, &state.catalog_error) {
        (_, Some(error)) => Box::new(
            el(
                "div",
                text(format!("The package catalog did not verify: {error}")),
            )
            .attr("class", "empty")
            .attr("role", "alert"),
        ),
        (Some(_), None) => Box::new(el("div", ()).attr("class", "empty-none")),
        (None, None) => {
            Box::new(el("div", text("No package catalog is loaded.")).attr("class", "empty"))
        }
    };
    let rows: Vec<Child> = state
        .catalog
        .as_ref()
        .map(|catalog| {
            catalog
                .packages()
                .iter()
                .enumerate()
                .map(|(index, package)| -> Child {
                    let selected = state.selected_package == Some(index);
                    Box::new(
                        button(
                            format!("{} — {:?}", package.package_id, package.state),
                            move |s: &mut DesktopState, _| s.select_package(index),
                        )
                        .attr("class", if selected { "row selected" } else { "row" })
                        .attr("aria-pressed", if selected { "true" } else { "false" })
                        .attr("data-package", package.package_id.clone()),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Box::new(el(
        "div",
        (
            heading(
                "Choose firmware",
                "Packages this publisher signed, verified against their payload hashes.",
            ),
            el("div", rows).attr("class", "rows").attr("role", "list"),
            catalog_note,
            el(
                "div",
                button("Review this firmware", |s: &mut DesktopState, _| {
                    s.request(Request::ConfirmFirmware)
                })
                .attr("class", "primary"),
            )
            .attr("class", "actions"),
        ),
    ))
}

// ------------------------------------------------------------ 3. review

fn ranges(label: &str, ranges: &[linkboy::FlashRange]) -> Child {
    if ranges.is_empty() {
        return field(label, "none");
    }
    field(
        label,
        ranges
            .iter()
            .map(|r| {
                format!(
                    "{:#010x}..{:#010x} ({} bytes)",
                    r.start,
                    r.start.saturating_add(r.length),
                    r.length
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn part_hashes(parts: &[linkboy::PackagePartIdentity]) -> String {
    parts
        .iter()
        .map(|part| {
            let address = part
                .offset
                .map(|offset| format!(" at {offset:#x}"))
                .unwrap_or_else(|| " in its container".into());
            format!("{}{}: {}", part.kind, address, part.sha256)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn review_changes(state: &DesktopState) -> Child {
    let view = state.view();
    let Some(review) = view.review else {
        return Box::new(el(
            "div",
            (
                heading("Review changes", "No approved plan to review."),
                el("div", text("Choose a device and a firmware package first."))
                    .attr("class", "empty"),
            ),
        ));
    };
    let impact = match review.state_impact {
        StateImpact::Preserved => "Preserved — your settings and keys survive",
        StateImpact::Replaced => "Replaced — settings and keys on the board are lost",
        StateImpact::Unknown => "Unknown — this package does not say",
    };
    Box::new(el(
        "div",
        (
            heading(
                "Review changes",
                "Exactly what will be written, and what it will cost you.",
            ),
            el("div", {
                let mut fields = vec![
                    field(
                        "Package",
                        format!("{} ({})", review.display_name, review.package_id),
                    ),
                    field("Version", review.version.clone()),
                    field("Publisher", review.publisher.clone()),
                    field("Artifact SHA-256", part_hashes(&review.package_parts)),
                    field("License", review.license.clone()),
                    field("Source", review.source_url.clone()),
                    field("Origin", review.origin_url.clone()),
                ];
                if let Some(signature) = &review.publisher_signature {
                    fields.extend([
                        field("Publisher signing key", signature.key_id.clone()),
                        field("Signed manifest", signature.signed_manifest_url.clone()),
                        field(
                            "Signed manifest SHA-256",
                            signature.signed_manifest_sha256.clone(),
                        ),
                    ]);
                }
                fields
            })
            .attr("class", "group")
            .attr("aria-label", "Package"),
            el(
                "div",
                (
                    field("Board", review.board.clone()),
                    field("Board revision", review.board_revision.clone()),
                    field(
                        "Board revision evidence",
                        review.board_revision_evidence.clone(),
                    ),
                    field("Route", review.route.clone()),
                    field(
                        "Helper",
                        format!("{} {}", review.helper, review.helper_version),
                    ),
                    field("Helper license", review.helper_license.clone()),
                    field("Helper source", review.helper_source_url.clone()),
                ),
            )
            .attr("class", "group")
            .attr("aria-label", "Route"),
            el(
                "div",
                (
                    ranges("Will write", &review.write_ranges),
                    ranges("Will preserve", &review.preserved_ranges),
                    field("State impact", impact),
                ),
            )
            .attr("class", "group")
            .attr("aria-label", "Changes"),
            el(
                "div",
                (
                    field("Before writing", review.recovery_before_write.clone()),
                    field("If it fails", review.recovery_after_failure.clone()),
                ),
            )
            .attr("class", "group")
            .attr("aria-label", "Recovery"),
            el(
                "div",
                button("Approve these changes", |s: &mut DesktopState, _| {
                    s.request(Request::ApproveChanges)
                })
                .attr("class", "primary"),
            )
            .attr("class", "actions"),
        ),
    ))
}

// ----------------------------------------------------------- 4. prepare

fn prepare_device(state: &DesktopState) -> Child {
    let view = state.view();
    let before = view
        .review
        .as_ref()
        .map(|r| r.recovery_before_write.clone())
        .unwrap_or_else(|| "No preparation instructions in this package.".into());
    Box::new(el(
        "div",
        (
            heading(
                "Prepare the device",
                "Do this now. After the next page it is too late to do it.",
            ),
            el("div", text(before))
                .attr("class", "instructions")
                .attr("role", "note"),
            field(
                "Device",
                view.device.clone().unwrap_or_else(|| "unknown".into()),
            ),
            field(
                "Package",
                view.package.clone().unwrap_or_else(|| "unknown".into()),
            ),
            el(
                "div",
                button("Start installing", |s: &mut DesktopState, _| {
                    s.request(Request::BeginInstall)
                })
                .attr("class", "primary"),
            )
            .attr("class", "actions"),
        ),
    ))
}

// ----------------------------------------------------------- 5. install

fn install(state: &DesktopState) -> Child {
    let pct = state.progress.map(|p| (p * 100.0).round() as u32);
    let notes: Vec<Child> = state
        .notes
        .iter()
        .map(|line| -> Child { Box::new(el("li", text(line.clone())).attr("class", "note")) })
        .collect();
    let bar: Child = match pct {
        Some(pct) => Box::new(
            el(
                "div",
                el("div", ())
                    .attr("class", "bar-fill")
                    .attr("style", format!("width:{pct}%;")),
            )
            .attr("class", "bar")
            .attr("role", "progressbar")
            .attr("aria-label", "Transfer")
            .attr("aria-valuenow", pct.to_string())
            .attr("aria-valuemin", "0")
            .attr("aria-valuemax", "100"),
        ),
        None => Box::new(el("div", ()).attr("class", "bar-none")),
    };
    Box::new(el(
        "div",
        (
            heading(
                "Install",
                "Leave the cable alone until this finishes or tells you what to do.",
            ),
            bar,
            el("ul", notes)
                .attr("class", "notes")
                .attr("role", "log")
                .attr("aria-label", "Installer events")
                .attr("aria-live", "polite"),
        ),
    ))
}

// ---------------------------------------------------- 6. verify/recover

fn verify_or_recover(state: &DesktopState) -> Child {
    let view = state.view();
    match view.result {
        Some(ReceiptResult::Complete) => {
            let receipt = state.receipt();
            let application = receipt
                .as_ref()
                .and_then(|r| r.application.as_ref())
                .map(|a| format!("{:?} {}", a.board, a.version))
                .unwrap_or_else(|| "not reported".into());
            Box::new(el(
                "div",
                (
                    heading("Verified", "The board came back and said what it is now."),
                    field("Result", "Complete"),
                    field("Running", application),
                    field(
                        "Package",
                        receipt
                            .as_ref()
                            .map(|r| r.package_id.clone())
                            .unwrap_or_default(),
                    ),
                    field(
                        "Artifact SHA-256",
                        receipt
                            .as_ref()
                            .map(|r| part_hashes(&r.package_parts))
                            .unwrap_or_default(),
                    ),
                    field(
                        "Board",
                        receipt
                            .as_ref()
                            .map(|r| format!("{:?} {}", r.board, r.board_revision))
                            .unwrap_or_default(),
                    ),
                    field(
                        "Board revision evidence",
                        receipt
                            .as_ref()
                            .map(|r| r.board_selection_evidence.clone())
                            .unwrap_or_default(),
                    ),
                ),
            ))
        }
        Some(ReceiptResult::ManualCheckRequired) => {
            let instruction = state
                .receipt()
                .and_then(|receipt| receipt.manual_check)
                .unwrap_or_else(|| {
                    "Use the upstream firmware's documented interface to verify it.".into()
                });
            Box::new(el(
                "div",
                (
                    heading(
                        "Manual check required",
                        "The verified package transferred, but this firmware has its own interface.",
                    ),
                    field("Result", "Manual check required"),
                    el("div", text(instruction))
                        .attr("class", "instructions")
                        .attr("role", "note"),
                ),
            ))
        }
        _ => {
            let detail = view
                .recovery_detail
                .clone()
                .unwrap_or_else(|| "The install did not finish.".into());
            let stage = state
                .recovery_stage()
                .map(|s| format!("It stopped {s}."))
                .unwrap_or_default();
            let instructions = state
                .recovery_instructions
                .as_ref()
                .cloned()
                .or_else(|| {
                    view.review
                        .as_ref()
                        .map(|r| r.recovery_after_failure.clone())
                })
                .unwrap_or_else(|| "No recovery instructions in this package.".into());
            Box::new(el(
                "div",
                (
                    heading(
                        "Recover",
                        "The board is in a known state and these steps get it back.",
                    ),
                    field("Result", "Recovery required"),
                    field(
                        "What happened",
                        format!("{stage} {detail}").trim().to_string(),
                    ),
                    el("div", text(instructions))
                        .attr("class", "instructions")
                        .attr("role", "note"),
                    field(
                        "Last known port",
                        state
                            .recovery
                            .as_ref()
                            .and_then(|f| f.last_known_port.clone())
                            .unwrap_or_else(|| "unknown".into()),
                    ),
                    field(
                        "Writing had started",
                        state
                            .recovery
                            .as_ref()
                            .map(|f| if f.write_started { "yes" } else { "no" })
                            .unwrap_or("unknown"),
                    ),
                ),
            ))
        }
    }
}
