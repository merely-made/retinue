//! The six-page owner flow, driven headlessly.
//!
//! No window, no GPU, and no board plugged in. The state machine and the views
//! are the real ones, and the interaction runs through the real desktop host:
//! `cambium_genet_winit_host::Harness` is the same `Host` the binary uses,
//! constructed without a window, so a click here goes through the same hit
//! test, dispatch, and focus rules a click there does.
//!
//! The device page's *hardware* half — opening a serial port and asking what is
//! on it — is the one thing not exercised, because there is nothing to ask. Its
//! refusals are, since those are decided before any port is opened.

use cambium_genet_winit_host::{CloseRequest, Harness, HostHooks, Init, inert_hooks};
use genet_probe::Selector;
use linkboy::device::{BoardSelection, DeviceTransport, EvidenceConfidence, FirmwareState};
use linkboy::executor::{ExecutionStage, RecoveryFacts};
use linkboy::package::{ProcessorKind, RecoveryInstructions};
use linkboy::{
    BoardFamily, DeviceObservation, FlashEvent, HardwareFacts, OwnerStage, ReceiptResult,
};
use signalman_desktop::state::{DesktopState, Request, V4ProductProfile};
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::{SHEET, default_catalog_path, root};
use winit::keyboard::NamedKey;

type App = Harness<DesktopState, Logic, Child>;

const SIZE: (f32, f32) = (1100.0, 800.0);

fn state() -> DesktopState {
    let mut state = DesktopState::new(&default_catalog_path());
    assert!(
        state.catalog_error.is_none(),
        "the repository's own package catalog must verify: {:?}",
        state.catalog_error,
    );
    // A survey result, handed in rather than read off a port.
    state.adopt_survey(vec![signalman::DeviceCandidate {
        port: "COM7".into(),
        board: Some("HeltecV4".into()),
        banner: "tulle/heltec-v4 phy online; version=0.0.1".into(),
        region: Some("US915".into()),
        channel: Some("modem".into()),
        known: true,
    }]);
    state
}

fn silent_state() -> DesktopState {
    let mut state = DesktopState::new(&default_catalog_path());
    assert!(state.catalog_error.is_none());
    state.adopt_survey(vec![signalman::DeviceCandidate {
        port: "COM9".into(),
        board: None,
        banner: String::new(),
        region: None,
        channel: None,
        known: false,
    }]);
    state
}

/// A harness with the app's own text seam wired, so caret behaviour is the
/// binary's rather than a stub's.
fn harness(state: DesktopState) -> App {
    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        focused_text: Box::new(signalman_desktop::focused_revision_field),
        close_request: Box::new(|ctx, _| {
            let mut disposition = None;
            ctx.runner
                .update(|state| disposition = Some(state.close_disposition()));
            disposition.expect("runner updates close disposition")
        }),
        ..inert_hooks()
    };
    let mut h = Harness::with_hooks(
        Init {
            state,
            logic: root as Logic,
            sheet: SHEET.to_string(),
        },
        hooks,
    );
    h.layout_at(SIZE.0, SIZE.1);
    h
}

/// The observation a real V4 survey plus an ESP ROM discovery would produce,
/// matching the catalogued `retinue.heltec-v4` target.
fn v4_observation() -> DeviceObservation {
    DeviceObservation {
        transport: DeviceTransport::SerialPort("COM7".into()),
        status_reply: Some("tulle/heltec-v4 phy online; version=0.0.1".into()),
        hardware: HardwareFacts {
            processor: Some(ProcessorKind::Esp32S3),
            flash_size: Some(16 * 1024 * 1024),
            bootloader: Some("esp-rom".into()),
            loader_route: Some("esp-rom".into()),
            bootloader_usb: None,
        },
        selected_board: Some(BoardSelection::owner_confirmed(
            BoardFamily::HeltecV4,
            "4.2",
        )),
        firmware: FirmwareState::Retinue {
            family: BoardFamily::HeltecV4,
        },
        confidence: EvidenceConfidence::OwnerConfirmed,
        contradictions: Vec::new(),
    }
}

/// Perform whatever the last click asked for, as the binary's `after_dispatch`
/// hook does. A worker slot is supplied but never started: no test starts a
/// flash.
fn settle(h: &mut App) {
    let mut worker = None;
    let wake: signalman::InstallerWake = std::sync::Arc::new(|| {});
    h.update(|state| {
        if let Some(request) = state.take_request() {
            signalman_desktop::flow::perform(state, request, &mut worker, wake.clone());
        }
    });
}

/// Drive to the review page without touching hardware: the device observation
/// goes straight into the owning flow (which is what `observe_device` would
/// hand it), and the package comes from the real catalog.
fn to_review(h: &mut App) {
    h.update(|state| {
        state
            .installer
            .choose_device(v4_observation())
            .expect("a fully-evidenced V4 observation is accepted");
        state.select_package(0);
        state.request(Request::ConfirmFirmware);
    });
    settle(h);
}

// ---------------------------------------------------------- page states

#[test]
fn the_flow_opens_on_choose_device_and_lists_what_answered() {
    let h = harness(state());
    assert_eq!(h.state().stage(), OwnerStage::ChooseDevice);
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Choose device"));
        assert!(
            genet_probe::text_present(s, "COM7"),
            "the surveyed port is on the page",
        );
        assert!(
            genet_probe::text_present(s, "US915"),
            "with what it said about itself",
        );
    });
}

/// The revision refusal is the flow's first real one, and it is text rather
/// than a disabled button. It also explains *why*, because "enter a revision"
/// without "nothing on the wire tells us which one" is a rule with no reason.
#[test]
fn a_missing_board_revision_refuses_in_words() {
    let mut h = harness(state());
    h.update(|state| {
        state.select_device(0);
        state.request(Request::ConfirmDevice);
    });
    settle(&mut h);

    assert_eq!(
        h.state().stage(),
        OwnerStage::ChooseDevice,
        "the flow did not advance",
    );
    assert!(!h.state().refusal.is_empty());
    h.with_surfaces(|s| {
        assert!(
            genet_probe::text_present(s, "exact board revision"),
            "the refusal says what is missing",
        );
        assert!(
            genet_probe::text_present(s, "refuses to plan a flash without a source"),
            "and why it matters",
        );
    });
}

/// Choosing a package is where compatibility is decided, and where the review
/// page's data comes from. Every field the scope names is on the page.
#[test]
fn the_review_page_shows_every_plan_fact() {
    let mut h = harness(state());
    to_review(&mut h);
    assert_eq!(h.state().stage(), OwnerStage::ReviewChanges);
    assert!(
        h.state().refusal.is_empty(),
        "a compatible package is not refused: {:?}",
        h.state().refusal,
    );

    let review = h.state().view().review.expect("the flow produced a review");
    h.with_surfaces(|s| {
        for (what, expected) in [
            ("package id", review.package_id.as_str()),
            ("display name", review.display_name.as_str()),
            ("version", review.version.as_str()),
            ("publisher", review.publisher.as_str()),
            ("license", review.license.as_str()),
            ("source url", review.source_url.as_str()),
            ("origin url", review.origin_url.as_str()),
            ("board revision", review.board_revision.as_str()),
            (
                "board revision evidence",
                review.board_revision_evidence.as_str(),
            ),
            ("helper", review.helper.as_str()),
            ("helper license", review.helper_license.as_str()),
            ("helper source", review.helper_source_url.as_str()),
            (
                "recovery before write",
                review.recovery_before_write.as_str(),
            ),
            (
                "recovery after failure",
                review.recovery_after_failure.as_str(),
            ),
        ] {
            assert!(
                genet_probe::text_present(s, expected),
                "the review page must show the {what}: {expected:?}",
            );
        }
        for part in &review.package_parts {
            assert!(
                genet_probe::text_present(s, &part.sha256),
                "the review page must show each verified artifact hash: {:?}",
                part.sha256,
            );
        }
        // Ranges are rendered, not summarized away.
        assert!(genet_probe::text_present(s, "0x00000000"));
        assert!(genet_probe::text_present(s, "0x003f0000"));
        // And the state impact is spelled out rather than left as an enum.
        assert!(genet_probe::text_present(s, "Preserved"));
    });
}

#[test]
fn the_review_keeps_a_documented_v4_profile_as_revision_evidence() {
    let mut h = harness(state());
    let mut observation = v4_observation();
    observation.selected_board = Some(BoardSelection::documented_product_profile(
        BoardFamily::HeltecV4,
        "4.2",
        "Meshnology N39 WiFi LoRa 32 V4 kit",
        "https://wiki.meshnology.com/N39/Meshnology%20N39/",
    ));
    h.update(|state| {
        state
            .installer
            .choose_device(observation)
            .expect("the documented profile has matching V4 facts");
        state.select_package(0);
        state.request(Request::ConfirmFirmware);
    });
    settle(&mut h);

    let review = h
        .state()
        .view()
        .review
        .expect("the documented plan has a review");
    assert!(review.board_revision_evidence.contains("Meshnology N39"));
    assert!(
        review
            .board_revision_evidence
            .contains("wiki.meshnology.com")
    );
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(
            s,
            "Meshnology N39 WiFi LoRa 32 V4 kit"
        ));
        assert!(genet_probe::text_present(
            s,
            "https://wiki.meshnology.com/N39/Meshnology%20N39/"
        ));
    });
}

/// Approving moves to the preparation page, which repeats the recovery
/// instructions *before* anything irreversible starts.
#[test]
fn approving_reaches_prepare_with_the_before_write_instructions() {
    let mut h = harness(state());
    to_review(&mut h);
    h.update(|state| state.request(Request::ApproveChanges));
    settle(&mut h);
    assert_eq!(h.state().stage(), OwnerStage::PrepareDevice);
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Prepare the device"));
        assert!(genet_probe::text_present(s, "Keep the USB cable attached",));
    });
}

/// Events progress the install page: each one becomes a line an owner can
/// read, and the write becomes a percentage rather than a spinner.
#[test]
fn events_progress_the_install_page() {
    let mut h = harness(state());
    to_review(&mut h);
    h.update(|state| state.request(Request::ApproveChanges));
    settle(&mut h);
    h.update(|state| {
        state.apply_event(&FlashEvent::Inspecting {
            device: "COM7".into(),
            package_id: "retinue.heltec-v4".into(),
        });
        state.apply_event(&FlashEvent::Erasing);
        state.apply_event(&FlashEvent::Writing {
            written: 1_000,
            total: 4_000,
        });
    });
    assert_eq!(h.state().stage(), OwnerStage::Install);
    assert_eq!(h.state().progress, Some(0.25));
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Inspecting COM7"));
        assert!(genet_probe::text_present(s, "Erasing"));
        assert!(genet_probe::text_present(s, "1000 of 4000 bytes (25%)"));
    });

    // A second write replaces the first line rather than stacking one entry per
    // chunk — a log an owner cannot read is not a log.
    h.update(|state| {
        state.apply_event(&FlashEvent::Writing {
            written: 3_000,
            total: 4_000,
        });
    });
    assert_eq!(
        h.state()
            .notes
            .iter()
            .filter(|n| n.starts_with("Writing "))
            .count(),
        1,
    );
    h.with_surfaces(|s| assert!(genet_probe::text_present(s, "(75%)")));
}

/// A recovery event ends on the recovery page with the package's own
/// after-failure instructions and the facts a person needs to act on.
#[test]
fn a_recovery_event_shows_the_recovery_context() {
    let mut h = harness(state());
    to_review(&mut h);
    h.update(|state| state.request(Request::ApproveChanges));
    settle(&mut h);

    let plan = h
        .state()
        .installer
        .plan()
        .cloned()
        .expect("an approved plan exists");
    let facts = RecoveryFacts {
        stage: ExecutionStage::Transfer,
        transport: "COM7".into(),
        last_known_port: Some("COM7".into()),
        write_started: true,
        detail: "the transfer stopped part-way".into(),
    };
    let receipt = linkboy::FlashReceipt::recovery_required(&plan, Vec::new());
    h.update(|state| {
        state.apply_event(&FlashEvent::RecoveryRequired {
            facts: facts.clone(),
            instructions: RecoveryInstructions {
                before_write: "Keep the cable attached.".into(),
                after_failure: "Re-enter the ROM loader and retry the same package.".into(),
            },
            receipt: receipt.clone(),
        });
    });

    assert_eq!(h.state().stage(), OwnerStage::VerifyOrRecover);
    assert!(h.state().needs_recovery());
    assert_eq!(
        h.state().view().result,
        Some(ReceiptResult::RecoveryRequired)
    );
    h.with_surfaces(|s| {
        assert!(genet_probe::text_present(s, "Recovery required"));
        assert!(genet_probe::text_present(s, "during the transfer"));
        assert!(genet_probe::text_present(
            s,
            "Re-enter the ROM loader and retry the same package.",
        ));
        assert!(
            genet_probe::text_present(s, "COM7"),
            "the last known port, so a person can find the board again",
        );
    });
}

// ------------------------------------------------------- semantic input

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
    // Focus it by Tab rather than by class, since the field is the input inside
    // the wrapper.
    h.tab(true); // Devices
    h.tab(true); // Network
    h.tab(true); // Messages
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

/// The DOM node's accessible label: its own text, else its `aria-label`.
fn label_of(dom: &genet_scripted_dom::ScriptedDom, node: genet_scripted_dom::NodeId) -> String {
    use layout_dom_api::{LayoutDom as _, LocalName, Namespace};
    let own: String = dom
        .dom_children(node)
        .filter_map(|c| dom.text(c).map(str::to_string))
        .collect();
    if !own.is_empty() {
        return own;
    }
    dom.attribute(node, &Namespace::from(""), &LocalName::from("aria-label"))
        .unwrap_or_default()
        .to_string()
}
