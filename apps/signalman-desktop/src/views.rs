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

use cambium::{AnyView, GenetCtx, GenetElement, button, el, text};
use linkboy::{OwnerStage, ReceiptResult, StateImpact};

use crate::state::{DesktopState, MESHNOLOGY_N39_DOCUMENTATION_URL, Request, SurveyState};

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

/// The application root: the trail, the current page, and the refusal panel.
pub fn root(state: &DesktopState) -> Child {
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
