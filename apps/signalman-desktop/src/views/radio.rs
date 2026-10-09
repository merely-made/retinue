//! The Radio section: availability captures and the observation collector.

use cambium::{button, el, text};
use signalman::observation::{Activity, Edge};

use crate::state::DesktopState;

use super::{Child, field, heading};

fn activity_label(activity: Activity) -> String {
    match activity {
        Activity::Listening {
            assignment,
            profile,
        } => format!(
            "listening, assignment {assignment}, profile {}",
            profile.map_or_else(|| "unknown".into(), |value| value.to_string())
        ),
        Activity::Transmit { work, profile } => format!(
            "transmitting, work {work}, profile {}",
            profile.map_or_else(|| "unknown".into(), |value| value.to_string())
        ),
        Activity::Quiet { cause } => format!("control/storage quiet: {cause:?}"),
        Activity::Sleep => "sleeping".into(),
    }
}

pub(super) fn radio_availability_page(state: &DesktopState) -> Child {
    let rows: Vec<Child> = state
        .availability
        .iter()
        .enumerate()
        .map(|(capture_index, capture)| {
            let mut boot_bounds = std::collections::BTreeMap::<u64, (u64, u64)>::new();
            for interval in &capture.timeline.intervals {
                for value in [interval.start_ms, interval.end_ms].into_iter().flatten() {
                    boot_bounds
                        .entry(interval.boot_id)
                        .and_modify(|(min, max)| {
                            *min = (*min).min(value);
                            *max = (*max).max(value);
                        })
                        .or_insert((value, value));
                }
            }
            let ticks: Vec<Child> = boot_bounds
                .iter()
                .map(|(boot, (min, max))| {
                    Box::new(el(
                        "div",
                        text(format!("boot {boot} ticks: {min} ms | {max} ms")),
                    )) as Child
                })
                .collect();
            let intervals: Vec<Child> = capture
                .timeline
                .intervals
                .iter()
                .map(|interval| {
                    let edge = if interval.edge == Edge::Complete {
                        "complete"
                    } else {
                        "incomplete"
                    };
                    let (boot_min, boot_max) = boot_bounds
                        .get(&interval.boot_id)
                        .copied()
                        .unwrap_or((0, 0));
                    let span = boot_max.saturating_sub(boot_min).max(1) as f64;
                    let left = interval.start_ms.map_or(0.0, |value| {
                        100.0 * value.saturating_sub(boot_min) as f64 / span
                    }).clamp(0.0, 98.0);
                    let requested_width = match (interval.start_ms, interval.end_ms) {
                        (Some(start), Some(end)) => {
                            (100.0 * end.saturating_sub(start) as f64 / span).max(2.0)
                        }
                        _ => 4.0,
                    };
                    let width = requested_width.min(100.0 - left).max(2.0);
                    let label = format!(
                        "boot {}: {} from {} to {} ({edge}{})",
                        interval.boot_id,
                        activity_label(interval.activity),
                        interval
                            .start_ms
                            .map_or_else(|| "unknown".into(), |v| format!("{v} ms")),
                        interval
                            .end_ms
                            .map_or_else(|| "unknown".into(), |v| format!("{v} ms")),
                        interval
                            .incomplete_reason
                            .map_or_else(String::new, |reason| format!(", {reason:?}")),
                    );
                    Box::new(
                        el(
                            "li",
                            (
                                el("div", text(""))
                                    .attr("class", format!("availability-marker {edge}"))
                                    .attr(
                                        "style",
                                        format!("margin-left: {left:.1}%; width: {width:.1}%"),
                                    )
                                    .attr("aria-hidden", "true"),
                                el("div", text(label.clone()))
                                    .attr("class", "availability-interval-label"),
                            ),
                        )
                            .attr("class", format!("availability-interval {edge}"))
                            .attr("aria-label", label),
                    ) as Child
                })
                .collect();
            let timeline_note = format!(
                "Source uptime is shown per boot. Host capture time: {} ms. The source reports {} omitted prefix entries; replay detects {} missing source records; {} captures, {} damaged frames, {} refusals.",
                capture.stored.captured_unix_ms,
                capture.stored.omitted_prefix_entries,
                capture.timeline.summary.missing_records,
                capture.timeline.summary.captures,
                capture.timeline.summary.damaged,
                capture.timeline.summary.refusals,
            );
            Box::new(
                el(
                    "section",
                    (
                        el("h2", text(String::from_utf8_lossy(capture.stored.bundle.device()).into_owned())),
                        button("Select capture", move |s: &mut DesktopState, _| {
                            s.select_availability(capture_index)
                        })
                        .attr("class", "secondary")
                        .attr(
                            "aria-pressed",
                            (state.selected_availability == Some(capture_index)).to_string(),
                        ),
                        el("div", text(capture.source.clone())).attr("class", "field-value"),
                        el("div", text(timeline_note)).attr("class", "availability-provenance"),
                        el("div", text("Time axis: board uptime within each boot; gaps and boot changes are uncertainty, not idle time."))
                            .attr("class", "availability-axis"),
                        el("div", ticks).attr("class", "availability-ticks"),
                        el("ol", intervals).attr("class", "availability-timeline"),
                    ),
                )
                .attr("class", "availability-board"),
            ) as Child
        })
        .collect();
    let content: Child = if rows.is_empty() {
        Box::new(el(
            "p",
            text("Load a versioned Signalman observation capture to inspect radio availability."),
        ))
    } else {
        Box::new(el("div", rows).attr("class", "availability-boards"))
    };
    let durable_label = if state.observation_durable {
        "Turn durable capture off"
    } else {
        "Turn durable capture on"
    };
    let collector_action: Child = if state.observation_collecting {
        Box::new(
            button("Stop collection", |s: &mut DesktopState, _| {
                s.request_observation_stop()
            })
            .attr("class", "secondary"),
        )
    } else {
        Box::new(
            button("Collect from selected device", |s: &mut DesktopState, _| {
                s.request_observation_start()
            })
            .attr("class", "primary"),
        )
    };
    let collector_controls = el(
        "section",
        (
            el(
                "label",
                (
                    el("div", text("Stable local device association")).attr("class", "field-label"),
                    el(
                        "div",
                        cambium::lens(
                            |input: &mut cambium::TextInput| cambium::text_field(input),
                            |s: &mut DesktopState| &mut s.observation_device_association,
                        ),
                    )
                    .attr("data-text-field", "observation-device-association"),
                ),
            )
            .attr("class", "field"),
            field(
                "Collection device",
                state
                    .device()
                    .map(|device| device.summary())
                    .unwrap_or_else(|| "Select a device on the Devices page".into()),
            ),
            collector_action,
        ),
    )
    .attr("class", "observation-collector");
    Box::new(
        el(
            "main",
            (
                heading(
                    "Radio availability",
                    "Measured board evidence, replayed without filling unknown time.",
                ),
                collector_controls,
                el(
                    "label",
                    (
                        el("div", text("Load capture path")).attr("class", "field-label"),
                        el(
                            "div",
                            cambium::lens(
                                |input: &mut cambium::TextInput| cambium::text_field(input),
                                |s: &mut DesktopState| &mut s.observation_load_path,
                            ),
                        )
                        .attr("data-text-field", "observation-load-path"),
                    ),
                )
                .attr("class", "field"),
                button("Load capture", |s: &mut DesktopState, _| {
                    s.request_observation_load()
                })
                .attr("class", "secondary"),
                el(
                    "label",
                    (
                        el("div", text("Export path (new file)")).attr("class", "field-label"),
                        el(
                            "div",
                            cambium::lens(
                                |input: &mut cambium::TextInput| cambium::text_field(input),
                                |s: &mut DesktopState| &mut s.observation_export_path,
                            ),
                        )
                        .attr("data-text-field", "observation-export-path"),
                    ),
                )
                .attr("class", "field"),
                button("Export selected capture", |s: &mut DesktopState, _| {
                    s.request_observation_export()
                })
                .attr("class", "primary"),
                button(durable_label, |s: &mut DesktopState, _| {
                    s.toggle_observation_durable()
                })
                .attr("class", "secondary"),
                field(
                    "Retention entry bound",
                    state.observation_retention_entries.to_string(),
                ),
                button("Change retained records", |s: &mut DesktopState, _| {
                    s.cycle_observation_entry_bound()
                })
                .attr("class", "secondary"),
                field(
                    "Retention byte bound",
                    state.observation_retention_bytes.to_string(),
                ),
                button("Change retained bytes", |s: &mut DesktopState, _| {
                    s.cycle_observation_byte_bound()
                })
                .attr("class", "secondary"),
                field(
                    "Retention age",
                    format!("{} ms", state.observation_retention_age_ms),
                ),
                button("Change retention age", |s: &mut DesktopState, _| {
                    s.cycle_observation_age_bound()
                })
                .attr("class", "secondary"),
                el(
                    "div",
                    text(state.observation_notice.clone().unwrap_or_default()),
                )
                .attr("role", "status"),
                content,
            ),
        )
        .attr("class", "page")
        .attr("role", "main")
        .attr("aria-label", "Radio availability"),
    )
}
