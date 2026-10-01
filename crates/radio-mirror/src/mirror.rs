//! A radio-face device in software: the firmware's `Controller` and renderer
//! over an RGBA framebuffer.

use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use radio_face::{
    Action, Button, Controller, HostSnapshot, InputEvent, InputProfile, LedIntent, LedSignal,
    LocalStatus, PressClassifier, Screen, Surface, Theme, WakeSource, led_intent, render,
};

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

/// One simulated radio. Page logic is the firmware's `Controller`; the few
/// lines of board glue it needs mirror the firmware `ui.rs` loops.
pub struct Mirror {
    surface: Surface,
    profile: InputProfile,
    theme: Theme<Rgb888>,
    controller: Controller,
    classifier: PressClassifier,
    local: LocalStatus,
    host: Option<HostSnapshot>,
    frame: RgbaFramebuffer<Rgb888>,
}

impl Mirror {
    pub fn new(surface: Surface, profile: InputProfile) -> Self {
        let controller = Controller::default();
        let local = LocalStatus {
            display_on: controller.display_on(),
            ..LocalStatus::default()
        };
        Self {
            surface,
            profile,
            theme: mono_theme(),
            controller,
            classifier: PressClassifier::default(),
            local,
            host: None,
            frame: RgbaFramebuffer::new(surface.size()),
        }
    }

    pub const fn surface(&self) -> Surface {
        self.surface
    }

    pub const fn profile(&self) -> InputProfile {
        self.profile
    }

    pub fn set_profile(&mut self, profile: InputProfile) {
        self.profile = profile;
    }

    pub fn set_theme(&mut self, theme: Theme<Rgb888>) {
        self.theme = theme;
    }

    pub const fn controller(&self) -> &Controller {
        &self.controller
    }

    pub const fn local(&self) -> &LocalStatus {
        &self.local
    }

    pub fn host(&self) -> Option<&HostSnapshot> {
        self.host.as_ref()
    }

    /// Replaces local status. `display_on` stays the controller's, as on the boards.
    pub fn set_local(&mut self, local: LocalStatus) {
        self.local = local;
        self.local.display_on = self.controller.display_on();
    }

    pub fn set_host(&mut self, host: Option<HostSnapshot>) {
        self.host = host;
    }

    /// Drops the snapshot once `elapsed_secs` exceeds its validity, as the boards do.
    pub fn age_host(&mut self, elapsed_secs: u32) -> bool {
        let expired = self.host.is_some_and(|host| !host.is_fresh(elapsed_secs));
        if expired {
            self.host = None;
        }
        expired
    }

    pub fn screen(&self) -> Screen {
        self.controller.screen(&self.local, self.host.as_ref())
    }

    /// A classified press, handled exactly as the firmware handles it.
    pub fn press(&mut self, event: InputEvent) -> Action {
        self.local.last_wake = WakeSource::Button;
        let action = self
            .controller
            .handle(self.profile, event, &self.local, self.host.as_ref());
        self.local.display_on = self.controller.display_on();
        action
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
        led_intent(&self.local, signal)
    }

    /// Whether the panel is lit: on, or forced on by a fault.
    pub fn panel_lit(&self) -> bool {
        self.controller.display_on() || self.local.fault.is_some()
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
            &self.local,
            self.host.as_ref(),
        );
        self.frame.as_rgba()
    }

    pub const fn frame(&self) -> &RgbaFramebuffer<Rgb888> {
        &self.frame
    }
}
