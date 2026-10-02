//! Press sequences through the mirror against `Controller` driven directly.

use radio_face::{
    Action, Button, Controller, Fault, HostSnapshot, InputEvent, InputProfile, LocalStatus,
    PressClassifier, Screen, Surface, Text, WakeSource,
};
use radio_mirror::{Mirror, input, names};

fn host() -> HostSnapshot {
    input::host_from_json(include_str!("../fixtures/receipts-host.json")).unwrap()
}

/// Steps the mirror and a bare controller in lockstep; returns the screens seen.
fn lockstep(
    profile: InputProfile,
    local: LocalStatus,
    host: Option<HostSnapshot>,
    script: &[&str],
) -> Vec<(Screen, Action)> {
    let mut mirror = Mirror::new(Surface::Oled128x64, profile);
    mirror.set_local(local);
    mirror.set_host(host);
    let mut controller = Controller::default();
    let mut seen = Vec::new();
    assert_eq!(mirror.screen(), controller.screen(&local, host.as_ref()));
    for name in script {
        let event = names::input_event(name).unwrap();
        let action = mirror.press(event);
        let expected = controller.handle(profile, event, &local, host.as_ref());
        assert_eq!(action, expected, "{name}");
        assert_eq!(
            mirror.screen(),
            controller.screen(&local, host.as_ref()),
            "{name}"
        );
        assert_eq!(mirror.controller(), &controller, "{name}");
        assert_eq!(
            mirror.local().display_on,
            controller.display_on(),
            "board glue"
        );
        assert_eq!(mirror.local().last_wake, WakeSource::Button, "board glue");
        mirror.render();
        seen.push((mirror.screen(), action));
    }
    seen
}

const TWO_BUTTON: &[&str] = &[
    "a-short", "a-short", "a-short", "a-short", "a-short", "a-short", "a-short", "b-short",
    "a-long", "a-short", // verify, then any key returns
    "chord", "a-short", "a-short", "b-short", // menu → verify
    "b-short", // leave verify
    "chord", "b-short", "b-short", // brightness twice
    "a-short", "b-short", // detail
    "b-long",  // close menu
    "chord", "a-short", "a-short", "a-short", "b-short", // display off
    "a-short", // wake, consumed
    "b-long", "chord", // off, wake
    "chord", "a-short", "a-short", "a-short", "a-short", "b-short", // reboot
    "chord", "a-short", "a-short", "a-short", "a-short", "a-short", "b-short", // back
];

#[test]
fn two_button_sequence_with_host_matches_the_controller() {
    let seen = lockstep(
        InputProfile::TwoButton,
        LocalStatus::default(),
        Some(host()),
        TWO_BUTTON,
    );
    let screens: Vec<String> = seen.iter().map(|(s, _)| names::screen_name(*s)).collect();
    assert_eq!(
        &screens[..8],
        [
            "power", "radio", "traffic", "identity", "links", "peers", "status", "peers"
        ]
    );
    assert!(screens.contains(&"verify".to_string()));
    assert!(screens.contains(&"display-off".to_string()));
    assert!(seen.iter().any(|(_, a)| *a == Action::RequestReboot));
    assert!(
        seen.iter()
            .any(|(_, a)| matches!(a, Action::BrightnessChanged(5)))
    );
}

#[test]
fn one_button_sequence_without_host_matches_the_controller() {
    let script = [
        "a-short", "a-short", "a-short", "a-short", "a-short", // four pages wrap
        "a-long", "a-short", "a-long", // menu, detail
        "a-long", "a-short", "a-short", "a-long",  // menu, display off
        "a-short", // wake
        "a-long", "a-short", "a-short", "a-short", "a-long", // back
        "b-short", "b-long", "chord", // not on a one-button board
    ];
    let seen = lockstep(
        InputProfile::OneButton,
        LocalStatus::default(),
        None,
        &script,
    );
    let screens: Vec<String> = seen.iter().map(|(s, _)| names::screen_name(*s)).collect();
    assert_eq!(
        &screens[..5],
        ["power", "radio", "traffic", "status", "power"]
    );
    assert!(
        screens
            .iter()
            .all(|s| !matches!(s.as_str(), "identity" | "links" | "peers"))
    );
}

#[test]
fn fault_preempts_and_ignores_presses() {
    let local = LocalStatus {
        fault: Some(Fault {
            code: 1,
            message: Text::from_truncated("SX1262 INIT FAILED"),
        }),
        ..LocalStatus::default()
    };
    let seen = lockstep(InputProfile::TwoButton, local, Some(host()), TWO_BUTTON);
    assert!(
        seen.iter()
            .all(|(s, a)| *s == Screen::Fault && *a == Action::None)
    );
}

#[test]
fn raw_edges_go_through_the_firmware_classifier() {
    let host = host();
    let local = LocalStatus::default();
    let mut mirror = Mirror::new(Surface::Tft240x135, InputProfile::TwoButton);
    mirror.set_host(Some(host));
    let mut classifier = PressClassifier::default();
    let mut controller = Controller::default();
    let edges = [
        (Button::A, true, 0),
        (Button::A, false, 100), // short
        (Button::B, true, 200),
        (Button::B, false, 300), // short
        (Button::A, true, 1_000),
        (Button::A, false, 1_700), // long: verify
        (Button::B, true, 2_000),
        (Button::B, false, 2_050), // leave
        (Button::A, true, 3_000),
        (Button::B, true, 3_010), // chord ...
        (Button::A, false, 4_000),
        (Button::B, false, 4_010), // ... menu
        (Button::A, true, 5_000),
        (Button::A, false, 5_100),
        (Button::B, true, 6_000),
        (Button::B, false, 6_700), // long: close menu
    ];
    let mut events = Vec::new();
    for (button, pressed, now) in edges {
        let got = mirror.edge(button, pressed, now);
        let expected = classifier.edge(button, pressed, now).map(|event| {
            (
                event,
                controller.handle(InputProfile::TwoButton, event, &local, Some(&host)),
            )
        });
        assert_eq!(got, expected, "{button:?} {pressed} {now}");
        assert_eq!(mirror.screen(), controller.screen(&local, Some(&host)));
        events.extend(got.map(|(event, _)| event));
    }
    assert_eq!(
        events,
        [
            InputEvent::AShort,
            InputEvent::BShort,
            InputEvent::ALong,
            InputEvent::BShort,
            InputEvent::Chord,
            InputEvent::AShort,
            InputEvent::BLong,
        ]
    );
}

#[test]
fn host_snapshots_expire_like_the_boards() {
    let mut mirror = Mirror::new(Surface::Oled128x64, InputProfile::TwoButton);
    mirror.set_host(Some(host()));
    assert!(!mirror.age_host(14));
    assert!(mirror.host().is_some());
    assert!(mirror.age_host(15));
    assert!(mirror.host().is_none());
}
