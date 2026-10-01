//! Kebab-case names for radio-face values, as a browser passes them.

use alloc::{format, string::String};

use radio_face::{
    Action, Button, DetailPolicy, InputEvent, InputProfile, LedIntent, LedSignal, MenuItem, Page,
    Screen, Surface,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownName {
    pub kind: &'static str,
    pub value: String,
}

impl core::fmt::Display for UnknownName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "unknown {} {:?}", self.kind, self.value)
    }
}

impl core::error::Error for UnknownName {}

fn unknown<T>(kind: &'static str, value: &str) -> Result<T, UnknownName> {
    Err(UnknownName {
        kind,
        value: value.into(),
    })
}

pub fn surface(value: &str) -> Result<Surface, UnknownName> {
    match value {
        "oled-128x64" => Ok(Surface::Oled128x64),
        "tft-240x135" => Ok(Surface::Tft240x135),
        _ => unknown("surface", value),
    }
}

pub const fn surface_name(value: Surface) -> &'static str {
    match value {
        Surface::Oled128x64 => "oled-128x64",
        Surface::Tft240x135 => "tft-240x135",
    }
}

pub fn input_profile(value: &str) -> Result<InputProfile, UnknownName> {
    match value {
        "one-button" => Ok(InputProfile::OneButton),
        "two-button" => Ok(InputProfile::TwoButton),
        _ => unknown("input profile", value),
    }
}

pub fn input_event(value: &str) -> Result<InputEvent, UnknownName> {
    match value {
        "a-short" => Ok(InputEvent::AShort),
        "a-long" => Ok(InputEvent::ALong),
        "b-short" => Ok(InputEvent::BShort),
        "b-long" => Ok(InputEvent::BLong),
        "chord" => Ok(InputEvent::Chord),
        _ => unknown("input event", value),
    }
}

pub const fn input_event_name(value: InputEvent) -> &'static str {
    match value {
        InputEvent::AShort => "a-short",
        InputEvent::ALong => "a-long",
        InputEvent::BShort => "b-short",
        InputEvent::BLong => "b-long",
        InputEvent::Chord => "chord",
    }
}

pub fn button(value: &str) -> Result<Button, UnknownName> {
    match value {
        "a" => Ok(Button::A),
        "b" => Ok(Button::B),
        _ => unknown("button", value),
    }
}

pub fn action_name(value: Action) -> String {
    match value {
        Action::None => "none".into(),
        Action::DisplayWoke => "display-woke".into(),
        Action::DisplayTurnedOff => "display-turned-off".into(),
        Action::BrightnessChanged(level) => format!("brightness-changed:{level}"),
        Action::DetailPolicyChanged(DetailPolicy::Minimal) => "detail-changed:minimal".into(),
        Action::DetailPolicyChanged(DetailPolicy::Named) => "detail-changed:named".into(),
        Action::RequestReboot => "request-reboot".into(),
    }
}

pub fn led_signal(value: &str) -> Result<LedSignal, UnknownName> {
    match value {
        "idle" => Ok(LedSignal::Idle),
        "activity" => Ok(LedSignal::Activity),
        "operation" => Ok(LedSignal::Operation),
        _ => unknown("LED signal", value),
    }
}

pub const fn led_intent_name(value: LedIntent) -> &'static str {
    match value {
        LedIntent::Off => "off",
        LedIntent::DoublePulse => "double-pulse",
        LedIntent::SlowPulse => "slow-pulse",
        LedIntent::FaultTriple => "fault-triple",
    }
}

const PAGES: [(Page, &str); 7] = [
    (Page::Status, "status"),
    (Page::Power, "power"),
    (Page::Radio, "radio"),
    (Page::Traffic, "traffic"),
    (Page::Identity, "identity"),
    (Page::Links, "links"),
    (Page::Peers, "peers"),
];

const MENU_ITEMS: [(MenuItem, &str); 6] = [
    (MenuItem::Brightness, "brightness"),
    (MenuItem::Detail, "detail"),
    (MenuItem::Verify, "verify"),
    (MenuItem::DisplayOff, "display-off"),
    (MenuItem::Reboot, "reboot"),
    (MenuItem::Back, "back"),
];

/// Pages by name; a menu is `menu:<item>:<index>`.
pub fn screen(value: &str) -> Result<Screen, UnknownName> {
    match value {
        "boot" => return Ok(Screen::Boot),
        "verify" => return Ok(Screen::Verify),
        "fault" => return Ok(Screen::Fault),
        "display-off" => return Ok(Screen::DisplayOff),
        _ => {}
    }
    if let Some((page, _)) = PAGES.iter().find(|(_, name)| *name == value) {
        return Ok(Screen::Page(*page));
    }
    let menu = value.strip_prefix("menu:").and_then(|rest| {
        let (item, index) = rest.rsplit_once(':')?;
        let (item, _) = MENU_ITEMS.iter().find(|(_, name)| *name == item)?;
        Some(Screen::Menu {
            selected: *item,
            selected_index: index.parse().ok()?,
        })
    });
    menu.map_or_else(|| unknown("screen", value), Ok)
}

pub fn screen_name(value: Screen) -> String {
    match value {
        Screen::Boot => "boot".into(),
        Screen::Verify => "verify".into(),
        Screen::Fault => "fault".into(),
        Screen::DisplayOff => "display-off".into(),
        Screen::Page(page) => PAGES
            .iter()
            .find(|(candidate, _)| *candidate == page)
            .map(|(_, name)| String::from(*name))
            .unwrap_or_default(),
        Screen::Menu {
            selected,
            selected_index,
        } => {
            let item = MENU_ITEMS
                .iter()
                .find(|(candidate, _)| *candidate == selected)
                .map_or("", |(_, name)| name);
            format!("menu:{item}:{selected_index}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_name_round_trips() {
        let mut screens = alloc::vec![
            Screen::Boot,
            Screen::Verify,
            Screen::Fault,
            Screen::DisplayOff
        ];
        screens.extend(PAGES.iter().map(|(page, _)| Screen::Page(*page)));
        screens.extend(
            MENU_ITEMS
                .iter()
                .enumerate()
                .map(|(index, (item, _))| Screen::Menu {
                    selected: *item,
                    selected_index: index as u8,
                }),
        );
        for value in screens {
            assert_eq!(screen(&screen_name(value)), Ok(value));
        }
        assert!(screen("route").is_err());
    }

    #[test]
    fn event_names_match_the_simulator_actions() {
        for name in ["a-short", "a-long", "b-short", "b-long", "chord"] {
            assert_eq!(input_event_name(input_event(name).unwrap()), name);
        }
    }
}
