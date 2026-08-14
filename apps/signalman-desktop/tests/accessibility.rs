//! What a screen reader is told about each page.
//!
//! This is the mechanical half of the accessibility pass: the projection the
//! AccessKit adapter is handed, asserted page by page. It cannot replace a
//! person listening to a real screen reader — announcement order, verbosity,
//! and whether the words make sense out loud are all things only a listener
//! knows — but it does hold the parts that silently rot: that every control
//! has a name, that the review page's facts are reachable, that a progress bar
//! reports a value, and that a refusal is an alert rather than decoration.

use accesskit::Role;
use cambium_genet_winit_host::{Harness, HostHooks, Init, inert_hooks};
use linkboy::device::{BoardSelection, DeviceTransport, EvidenceConfidence, FirmwareState};
use linkboy::package::ProcessorKind;
use linkboy::{BoardFamily, DeviceObservation, FlashEvent, HardwareFacts};
use signalman_desktop::state::{DesktopState, Request};
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::{SHEET, default_catalog_path, root};

type App = Harness<DesktopState, Logic, Child>;

fn harness() -> App {
    let mut state = DesktopState::new(&default_catalog_path());
    state.adopt_survey(vec![signalman::DeviceCandidate {
        port: "COM7".into(),
        board: Some("HeltecV4".into()),
        banner: "tulle/heltec-v4 phy online; version=0.0.1".into(),
        region: Some("US915".into()),
        channel: Some("modem".into()),
        known: true,
    }]);
    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        focused_text: Box::new(signalman_desktop::focused_revision_field),
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
    h.layout_at(1100.0, 800.0);
    h
}

fn silent_harness() -> App {
    let mut state = DesktopState::new(&default_catalog_path());
    state.adopt_survey(vec![signalman::DeviceCandidate {
        port: "COM9".into(),
        board: None,
        banner: String::new(),
        region: None,
        channel: None,
        known: false,
    }]);
    state.select_device(0);
    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        focused_text: Box::new(signalman_desktop::focused_revision_field),
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
    h.layout_at(1100.0, 800.0);
    h
}

/// Every projected node's role and accessible name.
fn announced(h: &mut App) -> Vec<(Role, String)> {
    let (tree, _) = h.a11y_tree();
    tree.nodes
        .iter()
        .map(|(_, node)| {
            (
                node.role(),
                node.label().map(|l| l.to_string()).unwrap_or_default(),
            )
        })
        .collect()
}

fn has_named_button(nodes: &[(Role, String)], name: &str) -> bool {
    nodes
        .iter()
        .any(|(role, label)| *role == Role::Button && label.contains(name))
}

/// Every control on the opening page announces as a button with a name. A
/// nameless control is one a reader can only describe as "button".
#[test]
fn every_control_on_the_device_page_has_a_name() {
    let mut h = harness();
    let nodes = announced(&mut h);
    assert!(
        has_named_button(&nodes, "COM7"),
        "the device row: {nodes:?}"
    );
    assert!(has_named_button(&nodes, "Rescan"), "{nodes:?}");
    assert!(has_named_button(&nodes, "Use this device"), "{nodes:?}");
    assert!(
        nodes
            .iter()
            .any(|(role, _)| matches!(role, Role::TextInput | Role::MultilineTextInput)),
        "the revision field projects as a text input: {nodes:?}",
    );
    assert!(
        !nodes
            .iter()
            .any(|(role, label)| *role == Role::Button && label.is_empty()),
        "no control is announced without a name: {nodes:?}",
    );
}

#[test]
fn silent_device_declarations_are_separate_named_controls() {
    let mut h = silent_harness();
    let nodes = announced(&mut h);
    assert!(
        has_named_button(&nodes, "This serial device is a V4"),
        "{nodes:?}"
    );
    assert!(
        has_named_button(&nodes, "This serial device is a T114"),
        "{nodes:?}"
    );
}

/// Focus is reported to the reader, so its virtual cursor lands where the
/// keyboard is rather than on the root.
#[test]
fn focus_is_reported_to_the_reader() {
    let mut h = harness();
    h.tab(true);
    let focused = h.focus().expect("Tab focused something");
    let (tree, map) = h.a11y_tree();
    let focused_dom = map.get(&tree.focus).copied();
    assert_eq!(
        focused_dom,
        Some(focused),
        "the projected focus is the DOM node the keyboard is on",
    );
}

/// The review page's facts are all reachable as text, and the refusal region is
/// an alert. A refusal a reader is never told about is the same as no refusal.
#[test]
fn the_review_page_and_a_refusal_are_both_announced() {
    let mut h = harness();
    // A refusal first, on the device page.
    h.update(|state| {
        state.select_device(0);
        state.request(Request::ConfirmDevice);
    });
    let mut worker = None;
    let wake: signalman::InstallerWake = std::sync::Arc::new(|| {});
    h.update(|state| {
        if let Some(request) = state.take_request() {
            signalman_desktop::flow::perform(state, request, &mut worker, wake.clone());
        }
    });
    let (tree, _) = h.a11y_tree();
    assert!(
        tree.nodes
            .iter()
            .any(|(_, node)| node.role() == Role::Alert),
        "the refusal projects as an alert",
    );

    // Then the review page.
    h.update(|state| {
        state
            .installer
            .choose_device(observation())
            .expect("observation accepted");
        state.select_package(0);
        state.request(Request::ConfirmFirmware);
    });
    h.update(|state| {
        if let Some(request) = state.take_request() {
            signalman_desktop::flow::perform(state, request, &mut worker, wake.clone());
        }
    });
    let review = h.state().view().review.expect("a review exists");
    let (tree, _) = h.a11y_tree();
    let text: String = tree
        .nodes
        .iter()
        .filter_map(|(_, node)| {
            node.value()
                .map(|v| v.to_string())
                .or_else(|| node.label().map(|l| l.to_string()))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut facts = vec![
        review.publisher.as_str(),
        review.helper_license.as_str(),
        review.recovery_after_failure.as_str(),
    ];
    facts.extend(review.package_parts.iter().map(|part| part.sha256.as_str()));
    for fact in facts {
        assert!(
            text.contains(fact),
            "a screen reader can reach {fact:?} on the review page",
        );
    }
}

/// The transfer bar reports a value, so a reader is told how far along it is
/// rather than only that something is happening.
#[test]
fn the_transfer_bar_reports_a_value() {
    let mut h = harness();
    let mut worker = None;
    let wake: signalman::InstallerWake = std::sync::Arc::new(|| {});
    h.update(|state| {
        state.installer.choose_device(observation()).unwrap();
        state.select_package(0);
        state.request(Request::ConfirmFirmware);
    });
    h.update(|state| {
        if let Some(request) = state.take_request() {
            signalman_desktop::flow::perform(state, request, &mut worker, wake.clone());
        }
    });
    h.update(|state| {
        state.installer.approve_changes().unwrap();
        state.apply_event(&FlashEvent::Writing {
            written: 1,
            total: 2,
        });
    });
    let (tree, _) = h.a11y_tree();
    let bar = tree
        .nodes
        .iter()
        .find(|(_, node)| node.role() == Role::ProgressIndicator)
        .map(|(_, node)| node);
    let bar = bar.expect("the transfer bar projects as a progress indicator");
    assert_eq!(
        bar.numeric_value(),
        Some(50.0),
        "and it carries how far along the transfer is",
    );
    assert_eq!(bar.label().map(|l| l.to_string()), Some("Transfer".into()));
}

fn observation() -> DeviceObservation {
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
