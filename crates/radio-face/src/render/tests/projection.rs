//! The text projection replays the same drawing calls as the pixels.

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyleBuilder},
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text as EgText},
};
use std::vec;
use std::vec::Vec;

use super::{Canvas, fixture, theme};
use crate::controller::{MenuItem, Page, Screen};
use crate::render::labels::menu_items;
use crate::render::{Painter, Surface, TextRole, paint, render};
use crate::status::{Fault, HostSnapshot, Text};

/// Every drawing call, kept so a screen can be replayed without `Pixels`.
enum Op {
    Clear(Rgb888),
    Text {
        role: TextRole,
        position: Point,
        visible: std::string::String,
        font: &'static MonoFont<'static>,
        color: Rgb888,
        background: Option<Rgb888>,
    },
    Fill(Rectangle, Rgb888),
    Line(Line, Rgb888),
}

struct Recorder(Vec<Op>);

impl Painter for Recorder {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn clear(&mut self, color: Rgb888) -> Result<(), Self::Error> {
        self.0.push(Op::Clear(color));
        Ok(())
    }

    fn text(
        &mut self,
        role: TextRole,
        position: Point,
        visible: &str,
        font: &'static MonoFont<'static>,
        color: Rgb888,
        background: Option<Rgb888>,
    ) -> Result<(), Self::Error> {
        self.0.push(Op::Text {
            role,
            position,
            visible: visible.into(),
            font,
            color,
            background,
        });
        Ok(())
    }

    fn fill(&mut self, area: Rectangle, color: Rgb888) -> Result<(), Self::Error> {
        self.0.push(Op::Fill(area, color));
        Ok(())
    }

    fn line(&mut self, line: Line, color: Rgb888) -> Result<(), Self::Error> {
        self.0.push(Op::Line(line, color));
        Ok(())
    }
}

/// Boot, every page, every menu position, verify, fault and display off.
fn every_screen(host: Option<&HostSnapshot>) -> Vec<Screen> {
    let mut screens = vec![Screen::Boot];
    for page in [
        Page::Status,
        Page::Power,
        Page::Radio,
        Page::Traffic,
        Page::Identity,
        Page::Links,
        Page::Peers,
    ] {
        screens.push(Screen::Page(page));
    }
    let (items, len) = menu_items(host);
    for index in 0..len {
        screens.push(Screen::Menu {
            selected: items[usize::from(index)],
            selected_index: index,
        });
    }
    screens.extend([Screen::Verify, Screen::Fault, Screen::DisplayOff]);
    screens
}

/// The projection's strings, drawn where the renderer draws its text,
/// reproduce the rendered frame exactly. A drawn run the projection lacks,
/// a row it adds, or a string that differs from the drawn one fails.
#[test]
fn text_rows_are_what_the_pixels_show() {
    let (local, host) = fixture();
    let mut faulted = local;
    faulted.fault = Some(Fault {
        code: 1,
        message: Text::from_truncated("SX1262 INIT FAILED"),
    });
    let cases = [(local, Some(&host)), (local, None), (faulted, Some(&host))];
    let mut checked = 0;
    for surface in [Surface::Oled128x64, Surface::Tft240x135] {
        for (local, host) in cases {
            for screen in every_screen(host) {
                let mut drawn = Canvas::new(surface.size());
                render(&mut drawn, surface, theme(), screen, &local, host).unwrap();
                let rows = crate::text::render_text(surface, screen, &local, host);
                assert!(!rows.is_empty(), "{surface:?} {screen:?} has no text");

                let mut recorder = Recorder(Vec::new());
                paint(&mut recorder, surface, theme(), screen, &local, host).unwrap();
                let mut replay = Canvas::new(surface.size());
                let mut texts = rows.iter();
                for op in &recorder.0 {
                    match op {
                        Op::Clear(color) => replay.clear(*color).unwrap(),
                        Op::Fill(area, color) => area
                            .into_styled(PrimitiveStyle::with_fill(*color))
                            .draw(&mut replay)
                            .unwrap(),
                        Op::Line(line, color) => line
                            .into_styled(PrimitiveStyle::with_stroke(*color, 1))
                            .draw(&mut replay)
                            .unwrap(),
                        Op::Text {
                            role,
                            position,
                            visible,
                            font,
                            color,
                            background,
                        } => {
                            if visible.is_empty() {
                                continue;
                            }
                            let row = texts.next().unwrap_or_else(|| {
                                panic!("{surface:?} {screen:?}: {visible:?} drawn, not projected")
                            });
                            assert_eq!(row.role, *role, "{surface:?} {screen:?}");
                            let mut style =
                                MonoTextStyleBuilder::new().font(font).text_color(*color);
                            if let Some(background) = background {
                                style = style.background_color(*background);
                            }
                            EgText::with_baseline(
                                &row.text,
                                *position,
                                style.build(),
                                Baseline::Top,
                            )
                            .draw(&mut replay)
                            .unwrap();
                        }
                    }
                }
                assert!(
                    texts.next().is_none(),
                    "{surface:?} {screen:?}: a projected row nothing drew"
                );
                assert!(
                    drawn.pixels == replay.pixels,
                    "{surface:?} {screen:?}: the text rows disagree with the pixels"
                );
                checked += 1;
            }
        }
    }
    // 2 surfaces x (17 + 16 + 17) screens.
    assert_eq!(checked, 100);
}

#[test]
fn readable_lines_join_labels_and_mark_the_selection() {
    let (local, host) = fixture();
    let status = crate::text::render_lines(
        Surface::Oled128x64,
        Screen::Page(Page::Status),
        &local,
        Some(&host),
    );
    let menu = crate::text::render_lines(
        Surface::Tft240x135,
        Screen::Menu {
            selected: MenuItem::Verify,
            selected_index: 2,
        },
        &local,
        Some(&host),
    );
    assert_eq!(
        status,
        [
            "STATUS, RAD OK",
            "BOARD: HELTEC V4",
            "FIRMWARE: PHY V10",
            "HOST: ATTACHED",
            "UPTIME: 4H 5M",
            "LOCAL MODEM TRUTH",
        ]
    );
    assert_eq!(
        menu,
        [
            "MENU, LOCAL",
            "BRIGHTNESS",
            "STATUS DETAIL",
            "VERIFY (SELECTED)",
            "DISPLAY OFF",
            "REBOOT",
            "MOVE / SELECT / BACK",
        ]
    );
}
