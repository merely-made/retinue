//! Owner page 1: choose a device.

use cambium::{button, el, text};

use crate::state::{DesktopState, MESHNOLOGY_N39_DOCUMENTATION_URL, Request, SurveyState};

use super::{Child, heading};

pub(super) fn choose_device(state: &DesktopState) -> Child {
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
                    // The `<label>` wraps the field: `text_field` generates no
                    // id, so `for` would name nothing.
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
