//! The text projection: each screen's drawn text as strings, in draw order.
//!
//! It runs the same painting code as [`render`](fn@crate::render), so a row
//! here is exactly a string the panel shows, clipped as the panel clips it.
//! For screen readers and alt text; feature `text`, which needs `alloc`.

use alloc::{format, string::String, vec::Vec};
use core::convert::Infallible;

use embedded_graphics::{
    mono_font::MonoFont,
    pixelcolor::BinaryColor,
    prelude::Point,
    primitives::{Line, Rectangle},
};

use crate::{
    controller::Screen,
    render::{Painter, Surface, TextRole, Theme, paint},
    status::{HostSnapshot, LocalStatus},
};

/// One run of text as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRow {
    pub role: TextRole,
    pub text: String,
}

/// The rows `screen` draws on `surface`, in draw order. Empty runs are skipped.
pub fn render_text(
    surface: Surface,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Vec<TextRow> {
    let mut rows = Rows(Vec::new());
    let theme = Theme::new(
        BinaryColor::Off,
        BinaryColor::On,
        BinaryColor::On,
        BinaryColor::On,
    );
    match paint(&mut rows, surface, theme, screen, local, host) {
        Ok(()) => rows.0,
        Err(never) => match never {},
    }
}

/// [`render_text`] as readable lines: a title joins its status (`STATUS, RAD OK`),
/// a label its value (`BOARD: HELTEC V4`), and the menu highlight is spelled out.
pub fn render_lines(
    surface: Surface,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Vec<String> {
    lines(&render_text(surface, screen, local, host))
}

pub fn lines(rows: &[TextRow]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rows = rows.iter().peekable();
    while let Some(row) = rows.next() {
        let next = rows.peek().map(|next| next.role);
        let line = match (row.role, next) {
            (TextRole::Title, Some(TextRole::Status)) => {
                format!(
                    "{}, {}",
                    row.text,
                    rows.next().map_or("", |status| status.text.as_str())
                )
            }
            (TextRole::Label, Some(TextRole::Value)) => {
                format!(
                    "{}: {}",
                    row.text,
                    rows.next().map_or("", |value| value.text.as_str())
                )
            }
            (TextRole::Selected, _) => format!("{} (SELECTED)", row.text),
            _ => row.text.clone(),
        };
        lines.push(line);
    }
    lines
}

struct Rows(Vec<TextRow>);

impl Painter for Rows {
    type Color = BinaryColor;
    type Error = Infallible;

    fn clear(&mut self, _color: BinaryColor) -> Result<(), Infallible> {
        Ok(())
    }

    fn text(
        &mut self,
        role: TextRole,
        _position: Point,
        visible: &str,
        _font: &'static MonoFont<'static>,
        _color: BinaryColor,
        _background: Option<BinaryColor>,
    ) -> Result<(), Infallible> {
        if !visible.is_empty() {
            self.0.push(TextRow {
                role,
                text: visible.into(),
            });
        }
        Ok(())
    }

    fn fill(&mut self, _area: Rectangle, _color: BinaryColor) -> Result<(), Infallible> {
        Ok(())
    }

    fn line(&mut self, _line: Line, _color: BinaryColor) -> Result<(), Infallible> {
        Ok(())
    }
}
