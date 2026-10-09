//! The application root, its sections, and the six owner pages.
//!
//! Everything a page shows comes from the flow's own projection; the review
//! page renders `FirmwareReview` field by field so the owner can check it.
//!
//! Refusals and warnings are visible states. No control is disabled without an
//! explanation: a step that cannot proceed says why, in Linkboy's words.

mod device;
mod firmware;
mod messages;
mod network;
mod radio;

use cambium::{AnyView, GenetCtx, GenetElement, button, el, text};
use linkboy::OwnerStage;

use crate::state::{DesktopSection, DesktopState};

use device::choose_device;
use firmware::{choose_firmware, install, prepare_device, review_changes, verify_or_recover};
use messages::messages_page;
use network::network_page;
use radio::radio_availability_page;

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

/// The application root: stable sections and the selected face.
pub fn root(state: &DesktopState) -> Child {
    let tabs: Vec<Child> = [
        ("Devices", DesktopSection::Devices),
        ("Network", DesktopSection::Network),
        ("Messages", DesktopSection::Messages),
        ("Radio", DesktopSection::Radio),
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
                    DesktopSection::Radio => radio_availability_page(state),
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
