//! Headless receipt for Signalman's five-section management shell.

use accesskit::Role;
use cambium_genet_winit_host::{Harness, HostHooks, Init, inert_hooks};
use genet_probe::Selector;
use linkboy::{BoardFamily, OwnerStage};
use signalman_desktop::state::{
    DesktopSection, DesktopState, LabelDensity, ManagementSettings, NetworkRequest, SurveyState,
};
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::{SHEET, default_catalog_path, root};
use winit::keyboard::NamedKey;

type App = Harness<DesktopState, Logic, Child>;

#[derive(Debug, PartialEq, Eq)]
struct DevicesReceipt {
    stage: OwnerStage,
    selected_device: Option<usize>,
    selected_package: Option<usize>,
    selected_board_family: Option<BoardFamily>,
    revision: String,
    uf2_volume: String,
    loader_record: String,
    survey: SurveyState,
    ports: Vec<String>,
    refusal: Vec<String>,
    notes: Vec<String>,
    install_running: bool,
}

fn receipt(state: &DesktopState) -> DevicesReceipt {
    DevicesReceipt {
        stage: state.stage(),
        selected_device: state.selected_device,
        selected_package: state.selected_package,
        selected_board_family: state.selected_board_family.clone(),
        revision: state.board_revision.text().to_owned(),
        uf2_volume: state.t114_uf2_volume.text().to_owned(),
        loader_record: state.t114_loader_record.text().to_owned(),
        survey: state.survey,
        ports: state
            .devices
            .iter()
            .map(|device| device.port.clone())
            .collect(),
        refusal: state.refusal.clone(),
        notes: state.notes.clone(),
        install_running: state.install_running,
    }
}

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
    state.select_device(0);
    state.board_revision = cambium::TextInput::new("4.2");
    state.t114_uf2_volume = cambium::TextInput::new("E:\\");
    state.t114_loader_record = cambium::TextInput::new("C:\\fixtures\\t114-loader.json");
    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        focused_text: Box::new(signalman_desktop::focused_revision_field),
        ..inert_hooks()
    };
    let mut harness = Harness::with_hooks(
        Init {
            state,
            logic: root as Logic,
            sheet: SHEET.to_owned(),
        },
        hooks,
    );
    harness.layout_at(1100.0, 800.0);
    harness
}

fn label_of(dom: &genet_scripted_dom::ScriptedDom, node: genet_scripted_dom::NodeId) -> String {
    use layout_dom_api::{LayoutDom as _, LocalName, Namespace};
    let own = dom
        .dom_children(node)
        .filter_map(|child| dom.text(child))
        .collect::<String>();
    if !own.is_empty() {
        return own;
    }
    dom.attribute(node, &Namespace::from(""), &LocalName::from("aria-label"))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn pointer_reaches_every_section_and_returning_to_devices_restores_the_exact_state() {
    let mut harness = harness();
    harness.update(|state| state.install_running = true);
    let before = receipt(harness.state());
    let sections = [
        ("Network", DesktopSection::Network),
        ("Messages", DesktopSection::Messages),
        ("Map", DesktopSection::Map),
        ("Browse", DesktopSection::Browse),
        ("Devices", DesktopSection::Devices),
    ];

    for (label, section) in sections {
        assert!(
            harness.click_on(&Selector::role("button").containing(label)),
            "{label} is pointer reachable"
        );
        assert_eq!(harness.state().section, section);
        harness.with_surfaces(|surfaces| {
            assert!(
                genet_probe::text_present(surfaces, label),
                "{label} names its selected section"
            );
        });
    }

    assert_eq!(receipt(harness.state()), before);
}

#[test]
fn unavailable_sections_name_their_actual_gate() {
    let mut harness = harness();
    let expectations = [
        (
            "Map",
            "Map is unavailable until owner placement records land.",
        ),
        (
            "Browse",
            "Browse is unavailable until document composition and source posture land.",
        ),
    ];

    for (section, wording) in expectations {
        assert!(harness.click_on(&Selector::role("button").containing(section)));
        harness.with_surfaces(|surfaces| {
            assert!(
                genet_probe::text_present(surfaces, wording),
                "{section} states the unmet gate instead of rendering sample data"
            );
        });
    }
}

#[test]
fn messages_face_persists_intent_before_carriage_and_names_its_actual_status() {
    let mut harness = harness();
    harness.update(|state| {
        state.set_message_local(signalman::message::MessagePeer::new([1; 16], Some([1; 32])));
        state.message_recipient = cambium::TextInput::new("02020202020202020202020202020202");
        state.message_draft = cambium::TextInput::new("Meet by the north gate");
    });
    assert!(harness.click_on(&Selector::role("button").containing("Messages")));
    assert!(harness.click_on(&Selector::role("button").containing("Queue message")));
    assert_eq!(harness.state().message_store.len(), 1);
    assert_eq!(harness.state().message_store.log_len(), 1);
    harness.with_surfaces(|surfaces| {
        assert!(genet_probe::text_present(
            surfaces,
            "Meet by the north gate"
        ));
        assert!(genet_probe::text_present(surfaces, "queued for station"));
        assert!(!genet_probe::text_present(surfaces, "delivered"));
    });

    harness.update(|state| {
        let id = state
            .message_store
            .records()
            .next()
            .expect("the queued message")
            .message
            .id();
        state.apply_message_event(signalman::message::MessageEvent::StatusChanged {
            id,
            status: signalman::message::MessageStatus::HandedToRadio {
                transport_id: [9; 32],
                mode: signalman::message::MessageTransport::Data,
            },
            observed_unix_ms: u64::MAX,
        });
    });
    harness.with_surfaces(|surfaces| {
        assert!(genet_probe::text_present(surfaces, "handed to radio"));
        assert!(!genet_probe::text_present(surfaces, "queued for station"));
    });
}

#[test]
fn keyboard_reaches_and_activates_all_five_sections_without_losing_devices() {
    let mut harness = harness();
    harness.update(|state| state.install_running = true);
    let before = receipt(harness.state());
    let expected = [
        ("Devices", DesktopSection::Devices),
        ("Network", DesktopSection::Network),
        ("Messages", DesktopSection::Messages),
        ("Map", DesktopSection::Map),
        ("Browse", DesktopSection::Browse),
    ];

    for (label, section) in expected {
        harness.tab(true);
        let focused = harness.focus().expect("section tab receives focus");
        assert_eq!(harness.with_dom(|dom| label_of(dom, focused)), label);
        harness.key_named(NamedKey::Enter);
        assert_eq!(harness.state().section, section);
    }

    harness.tab(true); // the unavailable Browse face has no lying controls; wrap to Devices
    let focused = harness.focus().expect("focus wraps to Devices");
    assert_eq!(harness.with_dom(|dom| label_of(dom, focused)), "Devices");
    harness.key_named(NamedKey::Enter);
    assert_eq!(harness.state().section, DesktopSection::Devices);
    assert_eq!(receipt(harness.state()), before);
}

#[test]
fn accesskit_names_all_five_primary_sections() {
    let mut harness = harness();
    let (tree, _) = harness.a11y_tree();
    let buttons = tree
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == Role::Button)
        .filter_map(|(_, node)| node.label().map(str::to_owned))
        .collect::<Vec<_>>();
    for section in ["Devices", "Network", "Messages", "Map", "Browse"] {
        assert!(
            buttons.iter().any(|label| label == section),
            "AccessKit names {section}: {buttons:?}"
        );
    }
}

#[test]
fn network_settings_are_owner_state_and_drive_the_available_runtime_seams() {
    let mut harness = harness();
    assert!(harness.click_on(&Selector::role("button").containing("Network")));

    assert!(harness.click_on(&Selector::role("button").containing("Shorter stale age")));
    assert_eq!(harness.state().management_settings.stale_age_minutes, 10);
    assert_eq!(harness.state().stale_policy().after.as_secs(), 10 * 60);

    assert!(harness.click_on(&Selector::role("button").containing("Keep more history")));
    assert_eq!(harness.state().announce_history_bound(), 512);

    assert!(harness.click_on(&Selector::role("button").containing("Stronger layout forces")));
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Reconcile(ref input))
            if (input.physics.force_strength - 1.25).abs() < f32::EPSILON
    ));

    assert!(harness.click_on(&Selector::role("button").containing("More damping")));
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Reconcile(ref input))
            if (input.physics.linear_damping - 3.0).abs() < f32::EPSILON
    ));

    assert!(harness.click_on(&Selector::role("button").containing("Hide node labels")));
    assert_eq!(
        harness.state().management_settings.label_density,
        LabelDensity::Hidden
    );
    assert!(!harness.state().network_swatch().show_labels);

    assert!(harness.click_on(&Selector::role("button").containing("Hide last-known devices")));
    assert!(!harness.state().management_settings.show_last_known);
    assert!(matches!(
        harness.state().pending_network,
        Some(NetworkRequest::Reconcile(_))
    ));

    assert!(harness.click_on(&Selector::role("button").containing("Reset management settings")));
    assert_eq!(
        harness.state().management_settings,
        ManagementSettings::default()
    );
}
