//! The board-loop glue, pinned as a press-and-time sequence per board.
//!
//! `PreLift` transcribes the glue each firmware's `ui.rs` `screen_task` ran
//! before Ruling 33 (heltec-v4-phy and t114-phy at retinue `5af9b44`; the two
//! loops are identical apart from the panel). `embassy_time::Instant` becomes a
//! millisecond clock; `Instant::elapsed().as_secs()` becomes `(now - at) / 1000`.
//! The expected sequences below were printed from `PreLift` before the lift.

use embedded_graphics::{
    Pixel,
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
};
use radio_face::{
    Action, BoardState, Controller, DetailPolicy, Fault, HostSnapshot, InputEvent, InputProfile,
    LocalStatus, NodeSummary, RadioState, Screen, Surface, Text, Theme, WakeSource, render,
};

#[derive(Clone, Copy, Debug)]
enum Event {
    Status(LocalStatus),
    Press(InputEvent),
    Host(HostSnapshot),
    Tick,
}

/// What one loop iteration left behind: the frame it drew (if any) and the state.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    drawn: Option<Screen>,
    action: Option<Action>,
    panel_on: bool,
    display_on: bool,
    last_wake: WakeSource,
    host: bool,
    uptime_secs: u32,
    digest: Option<u64>,
}

impl Step {
    fn line(&self) -> String {
        format!(
            "drawn={} action={} panel={} display_on={} wake={:?} host={} up={}",
            self.drawn
                .map_or("-".into(), |screen| format!("{screen:?}")),
            self.action
                .map_or("-".into(), |action| format!("{action:?}")),
            on(self.panel_on),
            self.display_on,
            self.last_wake,
            if self.host { "fresh" } else { "none" },
            self.uptime_secs,
        )
    }
}

fn on(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

/// The board's frame buffer reduced to a digest of lit pixels.
struct Frame {
    size: Size,
    lit: Vec<bool>,
}

impl Frame {
    fn new(surface: Surface) -> Self {
        let size = surface.size();
        Self {
            size,
            lit: vec![false; (size.width * size.height) as usize],
        }
    }

    fn digest(&self) -> u64 {
        self.lit.iter().fold(0xcbf29ce484222325, |hash, lit| {
            (hash ^ u64::from(*lit)).wrapping_mul(0x100000001b3)
        })
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for Frame {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x < 0
                || point.y < 0
                || point.x >= self.size.width as i32
                || point.y >= self.size.height as i32
            {
                continue;
            }
            let index = point.y as usize * self.size.width as usize + point.x as usize;
            self.lit[index] = color.is_on();
        }
        Ok(())
    }
}

/// Both boards' theme (V4 `ui.rs:336`, T114 `ui.rs:358`).
const THEME: Theme<BinaryColor> = Theme::new(
    BinaryColor::Off,
    BinaryColor::On,
    BinaryColor::On,
    BinaryColor::On,
);

fn draw(surface: Surface, screen: Screen, local: &LocalStatus, host: Option<&HostSnapshot>) -> u64 {
    let mut frame = Frame::new(surface);
    render(&mut frame, surface, THEME, screen, local, host).unwrap();
    frame.digest()
}

/// The pre-lift glue, line for line from the firmware loop.
struct PreLift {
    surface: Surface,
    controller: Controller,
    local: LocalStatus,
    active_host: Option<(HostSnapshot, u64)>,
    panel_on: bool,
}

impl PreLift {
    fn new(surface: Surface, initial: LocalStatus) -> Self {
        Self {
            surface,
            controller: Controller::default(),
            local: initial,
            active_host: None,
            panel_on: true,
        }
    }

    fn refresh_clock(&mut self, now: u64) {
        self.local.uptime_secs = (now / 1000).min(u64::from(u32::MAX)) as u32;
    }

    fn fresh_host(&mut self, now: u64) -> Option<HostSnapshot> {
        if self.active_host.as_ref().is_some_and(|(snapshot, at)| {
            !snapshot.is_fresh(((now - at) / 1000).min(u64::from(u32::MAX)) as u32)
        }) {
            self.active_host = None;
        }
        self.active_host.as_ref().map(|(snapshot, _)| *snapshot)
    }

    fn current(&self, host: Option<&HostSnapshot>) -> Screen {
        self.controller.screen(&self.local, host)
    }

    fn step(&mut self, event: Event, now: u64) -> Step {
        let mut drawn = None;
        let mut action = None;
        let host_seen: Option<HostSnapshot>;
        match event {
            Event::Status(status) => {
                self.local = status;
                self.refresh_clock(now);
                self.local.display_on = self.controller.display_on();
                let host = self.fresh_host(now);
                if self.local.fault.is_some() {
                    self.panel_on = true;
                    drawn = Some(self.current(host.as_ref()));
                } else if self.controller.display_on() {
                    drawn = Some(self.current(host.as_ref()));
                } else {
                    self.panel_on = false;
                }
                host_seen = host;
            }
            Event::Press(input) => {
                self.refresh_clock(now);
                self.local.last_wake = WakeSource::Button;
                let host = self.fresh_host(now);
                let handled = self.controller.handle(
                    InputProfile::OneButton,
                    input,
                    &self.local,
                    host.as_ref(),
                );
                self.local.display_on = self.controller.display_on();
                action = Some(handled);
                host_seen = host;
                match handled {
                    Action::DisplayWoke => self.panel_on = true,
                    Action::DisplayTurnedOff => {
                        drawn = Some(Screen::DisplayOff);
                        if self.local.fault.is_none() {
                            self.panel_on = false;
                        }
                    }
                    _ => {}
                }
                if handled != Action::DisplayTurnedOff
                    && (self.controller.display_on() || self.local.fault.is_some())
                {
                    drawn = Some(self.current(host.as_ref()));
                }
            }
            Event::Host(snapshot) => {
                self.active_host = Some((snapshot, now));
                let host = Some(snapshot);
                if self.controller.display_on() || self.local.fault.is_some() {
                    drawn = Some(self.current(host.as_ref()));
                }
                host_seen = host;
            }
            Event::Tick => {
                self.refresh_clock(now);
                let host = self.fresh_host(now);
                if self.local.fault.is_some() {
                    self.panel_on = true;
                    drawn = Some(self.current(host.as_ref()));
                } else if self.controller.display_on() {
                    drawn = Some(self.current(host.as_ref()));
                }
                host_seen = host;
            }
        }
        let host = host_seen;
        Step {
            drawn,
            action,
            panel_on: self.panel_on,
            display_on: self.local.display_on,
            last_wake: self.local.last_wake,
            host: self.active_host.is_some(),
            uptime_secs: self.local.uptime_secs,
            digest: drawn.map(|screen| draw(self.surface, screen, &self.local, host.as_ref())),
        }
    }
}

/// The firmware loops after the lift: the same branches over [`BoardState`].
struct PostLift {
    surface: Surface,
    face: BoardState<u64>,
    panel_on: bool,
}

impl PostLift {
    fn new(surface: Surface, initial: LocalStatus) -> Self {
        Self {
            surface,
            face: BoardState::new(InputProfile::OneButton, initial),
            panel_on: true,
        }
    }

    fn tick(&mut self, now: u64) {
        self.face
            .set_uptime((now / 1000).min(u64::from(u32::MAX)) as u32);
        self.face
            .expire_host(|at| ((now - at) / 1000).min(u64::from(u32::MAX)) as u32);
    }

    fn step(&mut self, event: Event, now: u64) -> Step {
        let mut drawn = None;
        let mut action = None;
        match event {
            Event::Status(status) => {
                self.face.set_local(status);
                self.tick(now);
                if self.face.local().fault.is_some() {
                    self.panel_on = true;
                    drawn = Some(self.face.screen());
                } else if self.face.controller().display_on() {
                    drawn = Some(self.face.screen());
                } else {
                    self.panel_on = false;
                }
            }
            Event::Press(input) => {
                self.tick(now);
                let handled = self.face.press(input);
                action = Some(handled);
                match handled {
                    Action::DisplayWoke => self.panel_on = true,
                    Action::DisplayTurnedOff => {
                        drawn = Some(Screen::DisplayOff);
                        if self.face.local().fault.is_none() {
                            self.panel_on = false;
                        }
                    }
                    _ => {}
                }
                if handled != Action::DisplayTurnedOff && self.face.panel_lit() {
                    drawn = Some(self.face.screen());
                }
            }
            Event::Host(snapshot) => {
                self.face.set_host(snapshot, now);
                if self.face.panel_lit() {
                    drawn = Some(self.face.screen());
                }
            }
            Event::Tick => {
                self.tick(now);
                if self.face.local().fault.is_some() {
                    self.panel_on = true;
                    drawn = Some(self.face.screen());
                } else if self.face.controller().display_on() {
                    drawn = Some(self.face.screen());
                }
            }
        }
        let local = self.face.local();
        Step {
            drawn,
            action,
            panel_on: self.panel_on,
            display_on: local.display_on,
            last_wake: local.last_wake,
            host: self.face.host().is_some(),
            uptime_secs: local.uptime_secs,
            digest: drawn.map(|screen| draw(self.surface, screen, local, self.face.host())),
        }
    }
}

fn status() -> LocalStatus {
    LocalStatus {
        board: Text::from_truncated("HELTEC V4"),
        firmware: Text::from_truncated("PHY V10"),
        radio: RadioState::Online,
        last_wake: WakeSource::Radio,
        ..LocalStatus::default()
    }
}

fn faulted() -> LocalStatus {
    LocalStatus {
        fault: Some(Fault {
            code: 7,
            message: Text::from_truncated("SX1262 BUSY"),
        }),
        ..status()
    }
}

fn host(valid_for_secs: u16) -> HostSnapshot {
    HostSnapshot {
        valid_for_secs,
        detail: DetailPolicy::Named,
        node: Some(NodeSummary {
            name: Text::from_truncated("HERALD"),
            ..NodeSummary::default()
        }),
        queue_depth: 2,
        ..HostSnapshot::default()
    }
}

/// Boot, a named host that expires mid-sequence, pages, the one-button menu,
/// display off and wake, status while dark, and a fault that forces the panel on.
fn script() -> Vec<(u64, Event)> {
    use InputEvent::{ALong, AShort};
    vec![
        (700, Event::Status(status())),
        (1_700, Event::Tick),
        (2_000, Event::Host(host(5))),
        (2_500, Event::Press(AShort)), // power: WAKE BUTTON
        (3_000, Event::Press(AShort)), // radio
        (3_200, Event::Press(AShort)), // traffic
        (3_400, Event::Press(AShort)), // identity (host named)
        (3_600, Event::Press(AShort)), // links
        (4_000, Event::Tick),
        (6_999, Event::Tick),             // host 4.999 s old: still fresh
        (7_000, Event::Tick),             // 5 s: expired, links falls back
        (7_200, Event::Press(AShort)),    // four pages again
        (7_400, Event::Press(ALong)),     // menu
        (7_600, Event::Press(AShort)),    // detail
        (7_800, Event::Press(AShort)),    // display off
        (8_000, Event::Press(ALong)),     // select: display off
        (8_500, Event::Status(status())), // dark: panel stays off
        (9_000, Event::Tick),
        (9_500, Event::Host(host(15))), // dark: no frame
        (10_000, Event::Press(AShort)), // wake, consumed
        (10_200, Event::Press(AShort)),
        (11_000, Event::Status(faulted())),
        (11_200, Event::Press(AShort)), // fault preempts
        (12_000, Event::Tick),
        (12_500, Event::Status(status())),
        (13_000, Event::Press(ALong)), // menu
        (13_200, Event::Press(AShort)),
        (13_400, Event::Press(AShort)), // verify (host named)
        (13_800, Event::Press(ALong)),
        (14_000, Event::Press(AShort)), // any key leaves verify
        (25_000, Event::Tick),          // host 15.5 s old: expired
    ]
}

fn run_pre_lift(surface: Surface) -> Vec<Step> {
    let mut board = PreLift::new(surface, status());
    script()
        .into_iter()
        .map(|(now, event)| board.step(event, now))
        .collect()
}

fn run_post_lift(surface: Surface) -> Vec<Step> {
    let mut board = PostLift::new(surface, status());
    script()
        .into_iter()
        .map(|(now, event)| board.step(event, now))
        .collect()
}

fn pinned(v4: &[Step], t114: &[Step]) {
    let lines: Vec<String> = v4.iter().map(Step::line).collect();
    assert_eq!(lines, EXPECTED, "V4 screen and state sequence");
    let t114_lines: Vec<String> = t114.iter().map(Step::line).collect();
    assert_eq!(t114_lines, EXPECTED, "T114 screen and state sequence");
    let digests: Vec<_> = v4
        .iter()
        .zip(t114)
        .map(|(a, b)| (a.digest, b.digest))
        .collect();
    assert_eq!(digests, EXPECTED_DIGESTS, "frames (V4, T114)");
}

/// State and screen per step, the same on both boards.
const EXPECTED: &[&str] = &[
    "drawn=Page(Status) action=- panel=on display_on=true wake=Radio host=none up=0",
    "drawn=Page(Status) action=- panel=on display_on=true wake=Radio host=none up=1",
    "drawn=Page(Status) action=- panel=on display_on=true wake=Radio host=fresh up=1",
    "drawn=Page(Power) action=None panel=on display_on=true wake=Button host=fresh up=2",
    "drawn=Page(Radio) action=None panel=on display_on=true wake=Button host=fresh up=3",
    "drawn=Page(Traffic) action=None panel=on display_on=true wake=Button host=fresh up=3",
    "drawn=Page(Identity) action=None panel=on display_on=true wake=Button host=fresh up=3",
    "drawn=Page(Links) action=None panel=on display_on=true wake=Button host=fresh up=3",
    "drawn=Page(Links) action=- panel=on display_on=true wake=Button host=fresh up=4",
    "drawn=Page(Links) action=- panel=on display_on=true wake=Button host=fresh up=6",
    "drawn=Page(Power) action=- panel=on display_on=true wake=Button host=none up=7",
    "drawn=Page(Radio) action=None panel=on display_on=true wake=Button host=none up=7",
    "drawn=Menu { selected: Brightness, selected_index: 0 } action=None panel=on display_on=true wake=Button host=none up=7",
    "drawn=Menu { selected: Detail, selected_index: 1 } action=None panel=on display_on=true wake=Button host=none up=7",
    "drawn=Menu { selected: DisplayOff, selected_index: 2 } action=None panel=on display_on=true wake=Button host=none up=7",
    "drawn=DisplayOff action=DisplayTurnedOff panel=off display_on=false wake=Button host=none up=8",
    "drawn=- action=- panel=off display_on=false wake=Radio host=none up=8",
    "drawn=- action=- panel=off display_on=false wake=Radio host=none up=9",
    "drawn=- action=- panel=off display_on=false wake=Radio host=fresh up=9",
    "drawn=Page(Radio) action=DisplayWoke panel=on display_on=true wake=Button host=fresh up=10",
    "drawn=Page(Traffic) action=None panel=on display_on=true wake=Button host=fresh up=10",
    "drawn=Fault action=- panel=on display_on=true wake=Radio host=fresh up=11",
    "drawn=Fault action=None panel=on display_on=true wake=Button host=fresh up=11",
    "drawn=Fault action=- panel=on display_on=true wake=Button host=fresh up=12",
    "drawn=Page(Traffic) action=- panel=on display_on=true wake=Radio host=fresh up=12",
    "drawn=Menu { selected: Brightness, selected_index: 0 } action=None panel=on display_on=true wake=Button host=fresh up=13",
    "drawn=Menu { selected: Detail, selected_index: 1 } action=None panel=on display_on=true wake=Button host=fresh up=13",
    "drawn=Menu { selected: Verify, selected_index: 2 } action=None panel=on display_on=true wake=Button host=fresh up=13",
    "drawn=Verify action=None panel=on display_on=true wake=Button host=fresh up=13",
    "drawn=Page(Traffic) action=None panel=on display_on=true wake=Button host=fresh up=14",
    "drawn=Page(Traffic) action=- panel=on display_on=true wake=Button host=none up=25",
];

/// Frame digests per step: V4 OLED, then T114 TFT.
const EXPECTED_DIGESTS: &[(Option<u64>, Option<u64>)] = &[
    (Some(18160049952021323517), Some(9124544534308506245)),
    (Some(13314275593380650464), Some(12613246436175619404)),
    (Some(13314275593380650464), Some(12613246436175619404)),
    (Some(450301681017224554), Some(3169246030964445222)),
    (Some(1519789078027114822), Some(1219915604048163854)),
    (Some(2737521021028013722), Some(11584363048511759213)),
    (Some(5930124777936748557), Some(7407672399751375860)),
    (Some(11745759724640926425), Some(17027802451113815786)),
    (Some(11745759724640926425), Some(17027802451113815786)),
    (Some(11745759724640926425), Some(17027802451113815786)),
    (Some(450301681017224554), Some(3169246030964445222)),
    (Some(1519789078027114822), Some(1219915604048163854)),
    (Some(14427592917449501246), Some(3924339869370144243)),
    (Some(14668124917498875586), Some(17143939835407410763)),
    (Some(17166508139741630366), Some(3711113040737189635)),
    (Some(10489225026763634916), Some(10874531872314569963)),
    (None, None),
    (None, None),
    (None, None),
    (Some(1519789078027114822), Some(1219915604048163854)),
    (Some(2737521021028013722), Some(11584363048511759213)),
    (Some(7826432301951488510), Some(501271628858555391)),
    (Some(7826432301951488510), Some(501271628858555391)),
    (Some(7826432301951488510), Some(501271628858555391)),
    (Some(2737521021028013722), Some(11584363048511759213)),
    (Some(14179779200469582547), Some(3992132678215811299)),
    (Some(14420311200518956887), Some(17211732644253077819)),
    (Some(3449198010477346015), Some(15750638880367807295)),
    (Some(9695754915646243998), Some(10642015654092755868)),
    (Some(2737521021028013722), Some(11584363048511759213)),
    (Some(10102776933767289397), Some(8342297553623273805)),
];

#[test]
fn pre_lift_sequence_is_pinned() {
    pinned(
        &run_pre_lift(Surface::Oled128x64),
        &run_pre_lift(Surface::Tft240x135),
    );
}

#[test]
fn board_state_reproduces_the_pre_lift_sequence() {
    let v4 = run_post_lift(Surface::Oled128x64);
    let t114 = run_post_lift(Surface::Tft240x135);
    assert_eq!(v4, run_pre_lift(Surface::Oled128x64));
    assert_eq!(t114, run_pre_lift(Surface::Tft240x135));
    pinned(&v4, &t114);
}
