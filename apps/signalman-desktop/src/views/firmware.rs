//! Owner pages 2-6: firmware, review, prepare, install, verify or recover.

use cambium::{button, el, text};
use linkboy::{ReceiptResult, StateImpact};

use crate::state::{DesktopState, Request};

use super::{Child, field, heading};

pub(super) fn choose_firmware(state: &DesktopState) -> Child {
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

pub(super) fn review_changes(state: &DesktopState) -> Child {
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

pub(super) fn prepare_device(state: &DesktopState) -> Child {
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

pub(super) fn install(state: &DesktopState) -> Child {
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

pub(super) fn verify_or_recover(state: &DesktopState) -> Child {
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
