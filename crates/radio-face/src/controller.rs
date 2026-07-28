use crate::status::{DetailPolicy, HostSnapshot, LocalStatus};

pub const LONG_PRESS_MS: u32 = 650;
pub const CHORD_PRESS_MS: u32 = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    A,
    B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    AShort,
    ALong,
    BShort,
    BLong,
    Chord,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputProfile {
    OneButton,
    #[default]
    TwoButton,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PressClassifier {
    a_down: Option<u32>,
    b_down: Option<u32>,
    chord_start: Option<u32>,
    chord_emitted: bool,
}

impl PressClassifier {
    pub fn edge(&mut self, button: Button, pressed: bool, now_ms: u32) -> Option<InputEvent> {
        if pressed {
            let target = match button {
                Button::A => &mut self.a_down,
                Button::B => &mut self.b_down,
            };
            if target.is_none() {
                *target = Some(now_ms);
            }
            if self.a_down.is_some() && self.b_down.is_some() && self.chord_start.is_none() {
                self.chord_start = Some(now_ms);
                self.chord_emitted = false;
            }
            return None;
        }

        if let Some(chord_start) = self.chord_start {
            match button {
                Button::A => self.a_down = None,
                Button::B => self.b_down = None,
            }
            let event = if !self.chord_emitted && now_ms.wrapping_sub(chord_start) >= CHORD_PRESS_MS
            {
                self.chord_emitted = true;
                Some(InputEvent::Chord)
            } else {
                None
            };
            if self.a_down.is_none() && self.b_down.is_none() {
                self.chord_start = None;
                self.chord_emitted = false;
            }
            return event;
        }

        let started = match button {
            Button::A => self.a_down.take(),
            Button::B => self.b_down.take(),
        }?;
        let long = now_ms.wrapping_sub(started) >= LONG_PRESS_MS;
        Some(match (button, long) {
            (Button::A, false) => InputEvent::AShort,
            (Button::A, true) => InputEvent::ALong,
            (Button::B, false) => InputEvent::BShort,
            (Button::B, true) => InputEvent::BLong,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Status,
    Power,
    Radio,
    Traffic,
    Identity,
    Links,
    Peers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Brightness,
    Detail,
    Verify,
    DisplayOff,
    Reboot,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Boot,
    Page(Page),
    Menu {
        selected: MenuItem,
        selected_index: u8,
    },
    Verify,
    Fault,
    DisplayOff,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    None,
    DisplayWoke,
    DisplayTurnedOff,
    BrightnessChanged(u8),
    DetailPolicyChanged(DetailPolicy),
    RequestReboot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Modal {
    Menu,
    Verify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controller {
    page_index: u8,
    menu_index: u8,
    modal: Option<Modal>,
    display_on: bool,
    brightness: u8,
    detail: DetailPolicy,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            page_index: 0,
            menu_index: 0,
            modal: None,
            display_on: true,
            brightness: 3,
            detail: DetailPolicy::Minimal,
        }
    }
}

impl Controller {
    pub const fn brightness(&self) -> u8 {
        self.brightness
    }

    pub const fn detail(&self) -> DetailPolicy {
        self.detail
    }

    pub const fn display_on(&self) -> bool {
        self.display_on
    }

    pub fn screen(&self, local: &LocalStatus, host: Option<&HostSnapshot>) -> Screen {
        if local.fault.is_some() {
            return Screen::Fault;
        }
        if !self.display_on {
            return Screen::DisplayOff;
        }
        match self.modal {
            Some(Modal::Verify) => Screen::Verify,
            Some(Modal::Menu) => {
                let menu = MenuSet::new(host);
                Screen::Menu {
                    selected: menu.get(self.menu_index),
                    selected_index: self.menu_index,
                }
            }
            None => {
                let pages = PageSet::new(host);
                Screen::Page(pages.get(self.page_index))
            }
        }
    }

    pub fn handle(
        &mut self,
        profile: InputProfile,
        event: InputEvent,
        local: &LocalStatus,
        host: Option<&HostSnapshot>,
    ) -> Action {
        if !self.display_on {
            self.display_on = true;
            self.modal = None;
            return Action::DisplayWoke;
        }
        if local.fault.is_some() {
            return Action::None;
        }

        match self.modal {
            Some(Modal::Verify) => {
                self.modal = None;
                return Action::None;
            }
            Some(Modal::Menu) => return self.handle_menu(profile, event, host),
            None => {}
        }

        let pages = PageSet::new(host);
        match (profile, event) {
            (_, InputEvent::AShort) => self.page_index = pages.next(self.page_index),
            (InputProfile::TwoButton, InputEvent::BShort) => {
                self.page_index = pages.previous(self.page_index)
            }
            (InputProfile::TwoButton, InputEvent::ALong) if has_identity(host) => {
                self.modal = Some(Modal::Verify)
            }
            (InputProfile::TwoButton, InputEvent::BLong) => {
                self.display_on = false;
                return Action::DisplayTurnedOff;
            }
            (InputProfile::TwoButton, InputEvent::Chord)
            | (InputProfile::OneButton, InputEvent::ALong) => self.open_menu(),
            _ => {}
        }
        Action::None
    }

    fn open_menu(&mut self) {
        self.menu_index = 0;
        self.modal = Some(Modal::Menu);
    }

    fn handle_menu(
        &mut self,
        profile: InputProfile,
        event: InputEvent,
        host: Option<&HostSnapshot>,
    ) -> Action {
        let menu = MenuSet::new(host);
        match (profile, event) {
            (_, InputEvent::AShort) => {
                self.menu_index = menu.next(self.menu_index);
                Action::None
            }
            (InputProfile::TwoButton, InputEvent::BShort)
            | (InputProfile::OneButton, InputEvent::ALong) => {
                self.select_menu(menu.get(self.menu_index), host)
            }
            (InputProfile::TwoButton, InputEvent::BLong)
            | (InputProfile::TwoButton, InputEvent::Chord) => {
                self.modal = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn select_menu(&mut self, item: MenuItem, host: Option<&HostSnapshot>) -> Action {
        match item {
            MenuItem::Brightness => {
                self.brightness = self.brightness % 5 + 1;
                Action::BrightnessChanged(self.brightness)
            }
            MenuItem::Detail => {
                self.detail = match self.detail {
                    DetailPolicy::Minimal => DetailPolicy::Named,
                    DetailPolicy::Named => DetailPolicy::Minimal,
                };
                Action::DetailPolicyChanged(self.detail)
            }
            MenuItem::Verify if has_identity(host) => {
                self.modal = Some(Modal::Verify);
                Action::None
            }
            MenuItem::Verify => Action::None,
            MenuItem::DisplayOff => {
                self.display_on = false;
                self.modal = None;
                Action::DisplayTurnedOff
            }
            MenuItem::Reboot => Action::RequestReboot,
            MenuItem::Back => {
                self.modal = None;
                Action::None
            }
        }
    }
}

fn has_identity(host: Option<&HostSnapshot>) -> bool {
    host.and_then(HostSnapshot::named_node).is_some()
}

#[derive(Clone, Copy)]
struct PageSet {
    items: [Page; 7],
    len: u8,
}

impl PageSet {
    fn new(host: Option<&HostSnapshot>) -> Self {
        let mut pages = Self {
            items: [
                Page::Status,
                Page::Power,
                Page::Radio,
                Page::Traffic,
                Page::Identity,
                Page::Links,
                Page::Peers,
            ],
            len: 4,
        };
        if has_identity(host) {
            pages.len = 7;
        }
        pages
    }

    fn get(&self, index: u8) -> Page {
        self.items[usize::from(index % self.len)]
    }

    fn next(&self, index: u8) -> u8 {
        (index + 1) % self.len
    }

    fn previous(&self, index: u8) -> u8 {
        (index + self.len - 1) % self.len
    }
}

#[derive(Clone, Copy)]
struct MenuSet {
    items: [MenuItem; 6],
    len: u8,
}

impl MenuSet {
    fn new(host: Option<&HostSnapshot>) -> Self {
        if has_identity(host) {
            Self {
                items: [
                    MenuItem::Brightness,
                    MenuItem::Detail,
                    MenuItem::Verify,
                    MenuItem::DisplayOff,
                    MenuItem::Reboot,
                    MenuItem::Back,
                ],
                len: 6,
            }
        } else {
            Self {
                items: [
                    MenuItem::Brightness,
                    MenuItem::Detail,
                    MenuItem::DisplayOff,
                    MenuItem::Reboot,
                    MenuItem::Back,
                    MenuItem::Back,
                ],
                len: 5,
            }
        }
    }

    fn get(&self, index: u8) -> MenuItem {
        self.items[usize::from(index % self.len)]
    }

    fn next(&self, index: u8) -> u8 {
        (index + 1) % self.len
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LedSignal {
    #[default]
    Idle,
    Activity,
    Operation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LedIntent {
    #[default]
    Off,
    DoublePulse,
    SlowPulse,
    FaultTriple,
}

pub const fn led_intent(local: &LocalStatus, signal: LedSignal) -> LedIntent {
    if local.fault.is_some() {
        return LedIntent::FaultTriple;
    }
    match signal {
        LedSignal::Idle => LedIntent::Off,
        LedSignal::Activity => LedIntent::DoublePulse,
        LedSignal::Operation => LedIntent::SlowPulse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{NodeSummary, Text};

    fn host() -> HostSnapshot {
        HostSnapshot {
            detail: DetailPolicy::Named,
            node: Some(NodeSummary {
                name: Text::from_truncated("HERALD"),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn timing_classifier_uses_settled_thresholds() {
        let mut classifier = PressClassifier::default();
        assert_eq!(classifier.edge(Button::A, true, 0), None);
        assert_eq!(
            classifier.edge(Button::A, false, LONG_PRESS_MS - 1),
            Some(InputEvent::AShort)
        );
        assert_eq!(classifier.edge(Button::A, true, 1000), None);
        assert_eq!(
            classifier.edge(Button::A, false, 1000 + LONG_PRESS_MS),
            Some(InputEvent::ALong)
        );

        assert_eq!(classifier.edge(Button::A, true, 2000), None);
        assert_eq!(classifier.edge(Button::B, true, 2010), None);
        assert_eq!(
            classifier.edge(Button::A, false, 2010 + CHORD_PRESS_MS),
            Some(InputEvent::Chord)
        );
        assert_eq!(classifier.edge(Button::B, false, 3000), None);
    }

    #[test]
    fn modem_pages_do_not_invent_node_truth() {
        let local = LocalStatus::default();
        let mut controller = Controller::default();
        for expected in [Page::Status, Page::Power, Page::Radio, Page::Traffic] {
            assert_eq!(controller.screen(&local, None), Screen::Page(expected));
            controller.handle(InputProfile::TwoButton, InputEvent::AShort, &local, None);
        }
        assert_eq!(controller.screen(&local, None), Screen::Page(Page::Status));
    }

    #[test]
    fn named_snapshot_adds_identity_links_and_peers() {
        let local = LocalStatus::default();
        let host = host();
        let mut controller = Controller::default();
        for _ in 0..4 {
            controller.handle(
                InputProfile::TwoButton,
                InputEvent::AShort,
                &local,
                Some(&host),
            );
        }
        assert_eq!(
            controller.screen(&local, Some(&host)),
            Screen::Page(Page::Identity)
        );
        controller.handle(
            InputProfile::TwoButton,
            InputEvent::AShort,
            &local,
            Some(&host),
        );
        assert_eq!(
            controller.screen(&local, Some(&host)),
            Screen::Page(Page::Links)
        );
    }

    #[test]
    fn one_button_menu_has_explicit_selection_grammar() {
        let local = LocalStatus::default();
        let mut controller = Controller::default();
        controller.handle(InputProfile::OneButton, InputEvent::ALong, &local, None);
        assert!(matches!(
            controller.screen(&local, None),
            Screen::Menu { .. }
        ));
        controller.handle(InputProfile::OneButton, InputEvent::AShort, &local, None);
        let action = controller.handle(InputProfile::OneButton, InputEvent::ALong, &local, None);
        assert_eq!(action, Action::DetailPolicyChanged(DetailPolicy::Named));
    }

    #[test]
    fn display_wake_is_consumed_and_fault_preempts_pages() {
        let mut local = LocalStatus::default();
        let mut controller = Controller::default();
        assert_eq!(
            controller.handle(InputProfile::TwoButton, InputEvent::BLong, &local, None,),
            Action::DisplayTurnedOff
        );
        assert_eq!(
            controller.handle(InputProfile::TwoButton, InputEvent::AShort, &local, None,),
            Action::DisplayWoke
        );
        assert_eq!(controller.screen(&local, None), Screen::Page(Page::Status));

        local.fault = Some(crate::status::Fault {
            code: 1,
            message: Text::from_truncated("SX1262 INIT"),
        });
        assert_eq!(controller.screen(&local, None), Screen::Fault);
    }

    #[test]
    fn healthy_idle_and_sleep_are_led_dark() {
        let mut local = LocalStatus::default();
        assert_eq!(led_intent(&local, LedSignal::Idle), LedIntent::Off);
        local.sleep = crate::status::SleepState::Sleeping;
        assert_eq!(led_intent(&local, LedSignal::Idle), LedIntent::Off);
        assert_eq!(
            led_intent(&local, LedSignal::Activity),
            LedIntent::DoublePulse
        );
    }
}
