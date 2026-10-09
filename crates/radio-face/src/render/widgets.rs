//! Header, field, ticker and clipped-text building blocks shared by every screen.

use core::fmt::Write as _;

use embedded_graphics::{mono_font::MonoFont, prelude::*, primitives::Line};

use super::layout::{Layout, Theme};
use super::painter::{Painter, TextRole};
use crate::status::{EventSource, HostSnapshot, LocalStatus, Text};

pub(super) fn header<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    title: &str,
    right: &str,
) -> Result<(), P::Error>
where
    P: Painter,
{
    draw_fit(
        target,
        TextRole::Title,
        Point::new(1, 1),
        layout.width * 2 / 3,
        title,
        layout.header_font,
        theme.accent,
        None,
    )?;
    let right_width = text_width(right, layout.header_font).min(layout.width / 2);
    draw_fit(
        target,
        TextRole::Status,
        Point::new(layout.width - right_width - 1, 1),
        right_width,
        right,
        layout.header_font,
        theme.muted,
        None,
    )?;
    target.line(
        Line::new(
            Point::new(0, layout.header_divider_y),
            Point::new(layout.width - 1, layout.header_divider_y),
        ),
        theme.foreground,
    )?;
    Ok(())
}

pub(super) fn field<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    column: i32,
    row: i32,
    label: &str,
    value: &str,
) -> Result<(), P::Error>
where
    P: Painter,
{
    let x = column * layout.column_width + 1;
    let y = if row == 0 {
        layout.body_y
    } else {
        layout.second_row_y
    };
    let width = layout.column_width - 3;
    draw_fit(
        target,
        TextRole::Label,
        Point::new(x, y),
        width,
        label,
        layout.label_font,
        theme.muted,
        None,
    )?;
    draw_fit(
        target,
        TextRole::Value,
        Point::new(x, y + layout.label_to_value),
        width,
        value,
        layout.value_font,
        theme.foreground,
        None,
    )
}

pub(super) fn ticker<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    value: &str,
) -> Result<(), P::Error>
where
    P: Painter,
{
    target.line(
        Line::new(
            Point::new(0, layout.ticker_divider_y),
            Point::new(layout.width - 1, layout.ticker_divider_y),
        ),
        theme.muted,
    )?;
    draw_fit(
        target,
        TextRole::Ticker,
        Point::new(1, layout.ticker_y),
        layout.width - 2,
        value,
        layout.ticker_font,
        theme.muted,
        None,
    )
}

pub(super) fn ticker_event<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
{
    if let Some(event) = host.and_then(|snapshot| snapshot.event.as_ref()) {
        let mut text = Text::<32>::empty();
        let _ = text.write_str(match event.source {
            EventSource::Local => "LOCAL ",
            EventSource::Host => "HOST ",
        });
        let _ = text.write_str(event.text.as_str());
        return ticker(target, layout, theme, text.as_str());
    }
    if let Some(rx) = local.last_rx {
        let mut text = Text::<32>::empty();
        let _ = write!(&mut text, "LOCAL RX {}B {}DBM", rx.frame_len, rx.rssi_dbm);
        ticker(target, layout, theme, text.as_str())
    } else {
        ticker(target, layout, theme, "NO RECENT EVENT")
    }
}

pub(super) fn centered<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    value: &str,
) -> Result<(), P::Error>
where
    P: Painter,
{
    let width = text_width(value, layout.value_font).min(layout.width);
    draw_fit(
        target,
        TextRole::Notice,
        Point::new(
            (layout.width - width) / 2,
            layout.body_y + (layout.ticker_divider_y - layout.body_y) / 2,
        ),
        width,
        value,
        layout.value_font,
        theme.foreground,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_fit<P>(
    target: &mut P,
    role: TextRole,
    position: Point,
    width: i32,
    value: &str,
    font: &'static MonoFont<'static>,
    color: P::Color,
    background: Option<P::Color>,
) -> Result<(), P::Error>
where
    P: Painter,
{
    if width <= 0 {
        return Ok(());
    }
    let character_width = font.character_size.width as usize;
    let max_characters = (width as usize) / character_width;
    let visible = &value[..value.len().min(max_characters)];
    target.text(role, position, visible, font, color, background)
}

fn text_width(value: &str, font: &MonoFont<'_>) -> i32 {
    value.len() as i32 * font.character_size.width as i32
}
