//! The six-page owner flow, driven headlessly.
//!
//! No window, no GPU, and no board plugged in. The state machine and views are
//! the real ones, driven through `cambium_genet_winit_host::Harness`, the same
//! host the binary uses, so clicks take the same hit test, dispatch, and focus
//! rules. Only the device page's serial probing is not exercised; its refusals
//! are decided before any port is opened.

mod page_states;
mod semantic_input;

use cambium_genet_winit_host::{Harness, HostHooks, Init, inert_hooks};
use linkboy::device::{BoardSelection, DeviceTransport, EvidenceConfidence, FirmwareState};
use linkboy::package::ProcessorKind;
use linkboy::{BoardFamily, DeviceObservation, HardwareFacts};
use signalman_desktop::state::{DesktopState, Request};
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::{default_catalog_path, root, sheet};

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
            sheet: sheet(),
            fonts: Vec::new(),
            images: Vec::new(),
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
        native_node_state: linkboy::device::NativeNodeState::Unknown,
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
