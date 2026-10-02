//! A radio-face device in software: the firmware's `Controller` and renderer
//! over an RGBA framebuffer.

use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use radio_face::{
    Action, BoardState, Button, Controller, HostSnapshot, InputEvent, InputProfile, LedIntent,
    LedSignal, LocalStatus, PressClassifier, Screen, Surface, Theme, led_intent, render,
    render_lines,
};

use alloc::{string::String, vec::Vec};

use crate::framebuffer::RgbaFramebuffer;

/// The boards' own palette: both firmwares render `BinaryColor`, Off black and On white.
pub const fn mono_theme() -> Theme<Rgb888> {
    Theme::new(Rgb888::BLACK, Rgb888::WHITE, Rgb888::WHITE, Rgb888::WHITE)
}

/// The palette `radio-face/examples/render_receipts.rs` uses per surface.
pub const fn receipt_theme(surface: Surface) -> Theme<Rgb888> {
    match surface {
        Surface::Oled128x64 => mono_theme(),
        Surface::Tft240x135 => Theme::new(
            Rgb888::BLACK,
            Rgb888::new(238, 242, 255),
            Rgb888::new(128, 139, 156),
            Rgb888::new(255, 176, 0),
        ),
    }
}

/// Theme from `0xRRGGBB` words.
pub const fn theme_from_rgb(
    background: u32,
    foreground: u32,
    muted: u32,
    accent: u32,
) -> Theme<Rgb888> {
    const fn color(value: u32) -> Rgb888 {
        Rgb888::new((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }
    Theme::new(
        color(background),
        color(foreground),
        color(muted),
        color(accent),
    )
}

/// Renders one screen, stateless.
pub fn render_rgba(
    surface: Surface,
    theme: Theme<Rgb888>,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> RgbaFramebuffer<Rgb888> {
    let mut frame = RgbaFramebuffer::new(surface.size());
    draw(&mut frame, surface, theme, screen, local, host);
    frame
}

/// Renders one screen to PNG bytes, for build-time images.
#[cfg(feature = "png")]
pub fn render_png(
    surface: Surface,
    theme: Theme<Rgb888>,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<alloc::vec::Vec<u8>, png::EncodingError> {
    let mut bytes = alloc::vec::Vec::new();
    render_rgba(surface, theme, screen, local, host).write_png(&mut bytes)?;
    Ok(bytes)
}

fn draw(
    frame: &mut RgbaFramebuffer<Rgb888>,
    surface: Surface,
    theme: Theme<Rgb888>,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) {
    match render(frame, surface, theme, screen, local, host) {
        Ok(()) => {}
        Err(never) => match never {},
    }
}

/// One simulated radio. Page logic is the firmware's `Controller`, and the
/// board glue around it is radio-face's `BoardState`, as on the boards.
pub struct Mirror {
    surface: Surface,
    theme: Theme<Rgb888>,
    face: BoardState<()>,
    classifier: PressClassifier,
    frame: RgbaFramebuffer<Rgb888>,
}

impl Mirror {
    pub fn new(surface: Surface, profile: InputProfile) -> Self {
        Self {
            surface,
            theme: mono_theme(),
            face: BoardState::new(profile, LocalStatus::default()),
            classifier: PressClassifier::default(),
            frame: RgbaFramebuffer::new(surface.size()),
        }
    }

    pub const fn surface(&self) -> Surface {
        self.surface
    }

    pub const fn profile(&self) -> InputProfile {
        self.face.profile()
    }

    pub fn set_profile(&mut self, profile: InputProfile) {
        self.face.set_profile(profile);
    }

    pub fn set_theme(&mut self, theme: Theme<Rgb888>) {
        self.theme = theme;
    }

    pub const fn controller(&self) -> &Controller {
        self.face.controller()
    }

    pub const fn local(&self) -> &LocalStatus {
        self.face.local()
    }

    pub fn host(&self) -> Option<&HostSnapshot> {
        self.face.host()
    }

    /// Replaces local status. `display_on` stays the controller's, as on the boards.
    pub fn set_local(&mut self, local: LocalStatus) {
        self.face.set_local(local);
    }

    pub fn set_host(&mut self, host: Option<HostSnapshot>) {
        match host {
            Some(snapshot) => self.face.set_host(snapshot, ()),
            None => self.face.clear_host(),
        }
    }

    /// Drops the snapshot once `elapsed_secs` reaches its validity, as the boards do.
    pub fn age_host(&mut self, elapsed_secs: u32) -> bool {
        self.face.expire_host(|()| elapsed_secs)
    }

    pub fn screen(&self) -> Screen {
        self.face.screen()
    }

    /// A classified press, handled exactly as the firmware handles it.
    pub fn press(&mut self, event: InputEvent) -> Action {
        self.face.press(event)
    }

    /// A raw button edge, classified by the firmware's `PressClassifier`.
    pub fn edge(
        &mut self,
        button: Button,
        pressed: bool,
        now_ms: u32,
    ) -> Option<(InputEvent, Action)> {
        let event = self.classifier.edge(button, pressed, now_ms)?;
        Some((event, self.press(event)))
    }

    pub fn led(&self, signal: LedSignal) -> LedIntent {
        led_intent(self.face.local(), signal)
    }

    /// Whether the panel is lit: on, or forced on by a fault.
    pub fn panel_lit(&self) -> bool {
        self.face.panel_lit()
    }

    /// Renders the current screen and returns its RGBA bytes.
    pub fn render(&mut self) -> &[u8] {
        let screen = self.screen();
        self.render_screen(screen)
    }

    /// Renders a given screen with this device's state, e.g. `Screen::Boot`.
    pub fn render_screen(&mut self, screen: Screen) -> &[u8] {
        draw(
            &mut self.frame,
            self.surface,
            self.theme,
            screen,
            self.face.local(),
            self.face.host(),
        );
        self.frame.as_rgba()
    }

    /// What the current screen says, one readable line per row, for a screen
    /// reader or alt text. Same rows as the pixels (radio-face's text projection).
    pub fn text(&self) -> Vec<String> {
        self.text_for(self.screen())
    }

    /// What a given screen says with this device's state.
    pub fn text_for(&self, screen: Screen) -> Vec<String> {
        render_lines(self.surface, screen, self.face.local(), self.face.host())
    }

    pub const fn frame(&self) -> &RgbaFramebuffer<Rgb888> {
        &self.frame
    }
}
