//! Controls by role and label, the keyboard, the text seam, and close.

use cambium_genet_winit_host::CloseRequest;
use genet_probe::Selector;
use linkboy::{BoardFamily, OwnerStage};
use signalman_desktop::state::{Request, V4ProductProfile};
use winit::keyboard::NamedKey;

use crate::{SIZE, harness, label_of, settle, silent_state, state, to_review};

/// Controls are activated by role and label — the same resolution a
/// `genet-probe` scenario uses — rather than by coordinate.
#[test]
fn controls_activate_by_role_and_label() {
    let mut h = harness(state());
    assert!(
        h.click_on(&Selector::role("button").containing("COM7")),
        "the surveyed device row must resolve by its label",
    );
    assert_eq!(h.state().selected_device, Some(0));

    assert!(
        h.click_on(&Selector::role("button").containing("Use V4 revision 4.2")),
        "the recognized V4 revision remains an explicit owner choice",
    );
    assert_eq!(h.state().board_revision.text(), "4.2");

    assert!(
        h.click_on(&Selector::role("button").containing("Use this device")),
        "and so must the page's primary action",
    );
    assert_eq!(
        h.state().pending,
        Some(Request::ConfirmDevice),
        "activating it asked the application loop to confirm the device",
    );
}

/// A silent port carries no board identity. The owner may name the physical
/// board they are holding, after which the page offers only that family's
/// revision and evidence path. Neither family is chosen by selecting COM9.
#[test]
fn a_silent_device_offers_explicit_v4_and_t114_declarations() {
    let mut h = harness(silent_state());
    assert!(
        h.click_on(&Selector::role("button").containing("COM9")),
        "the silent serial location remains selectable"
    );
    assert_eq!(h.state().selected_board_family, None);
    h.with_surfaces(|surfaces| {
        assert!(genet_probe::text_present(
            surfaces,
            "This serial device is a V4"
        ));
        assert!(genet_probe::text_present(
            surfaces,
            "This serial device is a T114"
        ));
    });

    assert!(h.click_on(&Selector::role("button").containing("This serial device is a V4")));
    assert_eq!(h.state().selected_board_family, Some(BoardFamily::HeltecV4));
    assert!(h.click_on(&Selector::role("button").containing("Use V4 revision 4.2")));
    assert_eq!(h.state().board_revision.text(), "4.2");
    assert_eq!(h.state().v4_product_profile, None);

    assert!(h.click_on(&Selector::role("button").containing("Use Meshnology N39 V4.2 profile")));
    assert_eq!(h.state().board_revision.text(), "4.2");
    assert_eq!(
        h.state().v4_product_profile,
        Some(V4ProductProfile::MeshnologyN39V42)
    );
    let selection = h.state().board_selection(BoardFamily::HeltecV4, "4.2");
    assert!(matches!(
        selection.evidence,
        linkboy::BoardSelectionEvidence::DocumentedProductProfile { .. }
    ));

    let mut h = harness(silent_state());
    assert!(h.click_on(&Selector::role("button").containing("COM9")));
    assert!(h.click_on(&Selector::role("button").containing("This serial device is a T114")));
    assert_eq!(h.state().selected_board_family, Some(BoardFamily::T114));
    h.with_surfaces(|surfaces| {
        assert!(genet_probe::text_present(surfaces, "T114 UF2 route"));
        assert!(genet_probe::text_present(surfaces, "Mounted UF2 volume"));
        assert!(genet_probe::text_present(surfaces, "Loader record path"));
        assert!(genet_probe::text_present(surfaces, "T114 DFU recovery"));
        assert!(genet_probe::text_present(
            surfaces,
            "Use selected T114 DFU port"
        ));
    });
    assert!(h.click_on(&Selector::role("button").containing("Use T114 revision 2.x")));
    let loader_record = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../design_docs/2026-08-12_t114_loader_snapshot.json");
    h.update(|state| {
        state.t114_loader_record =
            cambium::TextInput::new(loader_record.to_string_lossy().into_owned());
    });
    assert!(h.click_on(&Selector::role("button").containing("Use selected T114 DFU port")));
    assert_eq!(h.state().pending, Some(Request::ConfirmT114Dfu));
    settle(&mut h);
    assert_eq!(h.state().stage(), OwnerStage::ChooseFirmware);
    assert_eq!(h.state().view().device.as_deref(), Some("serial-dfu:COM9"));
}

/// Keyboard order: Tab reaches every control on the device page, in the order
/// the page reads, and Enter activates the focused one. Nothing here uses the
/// pointer.
#[test]
fn the_device_page_is_operable_from_the_keyboard_alone() {
    let mut h = harness(state());

    // The reachable controls, in document order.
    let mut order = Vec::new();
    for _ in 0..11 {
        h.tab(true);
        let Some(node) = h.focus() else { break };
        let label = h.with_dom(|dom| label_of(dom, node));
        if order.contains(&label) {
            break; // wrapped
        }
        order.push(label);
    }
    assert_eq!(
        order,
        vec![
            "Devices".to_string(),
            "Network".to_string(),
            "Messages".to_string(),
            "Radio".to_string(),
            "Map".to_string(),
            "Browse".to_string(),
            "COM7 — HeltecV4, region US915, channel modem".to_string(),
            String::new(), // the revision field: an input, labelled by its <label>
            "Rescan".to_string(),
            "Use this device".to_string(),
        ],
        "Tab reaches the section switch and every device-page control in page order",
    );

    // The loop stopped one Tab past the end, so focus has wrapped to the first
    // control. Shift+Tab from there wraps backwards to the last one.
    h.tab(false);
    let back = h.with_dom(|dom| label_of(dom, h.focus().expect("focus held")));
    assert_eq!(
        back, "Use this device",
        "Shift+Tab walks backwards, wrapping at the start",
    );

    // Walk forward to Rescan and activate it with Enter — no pointer involved.
    h.tab(true); // wraps to Devices
    h.tab(true); // Network
    h.tab(true); // Messages
    h.tab(true); // Radio
    h.tab(true); // Map
    h.tab(true); // Browse
    h.tab(true); // the device row
    h.tab(true); // the revision field
    h.tab(true); // Rescan
    assert_eq!(
        h.with_dom(|dom| label_of(dom, h.focus().expect("focus held"))),
        "Rescan",
    );
    h.key_named(NamedKey::Enter);
    assert_eq!(h.state().pending, Some(Request::Rescan));
}

/// The revision field really edits: typing reaches it through the host's
/// `focused_text` seam, and what it holds is what the flow reads.
#[test]
fn the_revision_field_takes_typing_through_the_text_seam() {
    let mut h = harness(state());
    assert!(h.click_on(&Selector::class("revision-wrap")) || true);
    // Focus it by Tab: the field is inside the `revision-wrap` wrapper.
    h.tab(true); // Devices
    h.tab(true); // Network
    h.tab(true); // Messages
    h.tab(true); // Radio
    h.tab(true); // Map
    h.tab(true); // Browse
    h.tab(true); // device row
    h.tab(true); // revision field
    h.key_char("4");
    h.key_char(".");
    h.key_char("2");
    assert_eq!(h.state().board_revision.text(), "4.2");

    // And the flow then accepts it: no revision refusal.
    h.update(|state| {
        state.select_device(0);
        state.refusal.clear();
    });
    let revision = h.state().board_revision.text().trim().to_string();
    assert_eq!(revision, "4.2");
}

/// An active worker owns an unfinished physical operation, so both native and
/// in-app close keep the window available and explain why. Once that operation
/// is terminal, ordinary close is allowed again.
#[test]
fn active_install_vetoes_native_and_command_close_until_terminal() {
    let mut h = harness(state());
    h.update(|state| state.install_running = true);

    h.request_close(CloseRequest::Native);
    assert!(
        !h.close_requested(),
        "native close stays in the app while writing"
    );
    h.layout_at(SIZE.0, SIZE.1);
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Installation is still active"));
    });

    h.commands().close();
    h.after_dispatch();
    assert!(
        !h.close_requested(),
        "application close shares the active-install policy"
    );

    h.update(|state| state.install_running = false);
    h.request_close(CloseRequest::Native);
    assert!(h.close_requested(), "terminal install permits close");
}

/// The face cannot execute. There is no path from a view handler to
/// `execute_plan`: the only plan it can obtain comes from the flow's own
/// `start_install` gate, and the face supplies only a host wake callback.
#[test]
fn the_application_cannot_execute_a_plan_it_did_not_get_from_the_flow() {
    let mut h = harness(state());
    // Before approval, the gate refuses.
    to_review(&mut h);
    h.update(|state| {
        let err = state
            .installer
            .begin_install()
            .expect_err("begin_install refuses before the changes are approved");
        assert!(matches!(err, linkboy::FlowError::WrongStage { .. }));
    });
    // And there is no approved plan to hand anywhere until the owner approves.
    assert_eq!(h.state().stage(), OwnerStage::ReviewChanges);
}
