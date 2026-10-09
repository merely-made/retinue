//! The screens outside the page cycle: boot, menu, verify, fault and display off.

use core::fmt::Write as _;

use embedded_graphics::{prelude::*, primitives::Rectangle};

use super::labels::{menu_items, menu_label, value_or_dash};
use super::layout::{Layout, Theme};
use super::painter::{Painter, TextRole};
use super::widgets::{centered, draw_fit, header, ticker};
use crate::{
    controller::MenuItem,
    status::{HostSnapshot, LocalStatus, Text},
};

pub(super) fn render_boot<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
) -> Result<(), P::Error>
where
    P: Painter,
{
    let title_y = if layout.height == 64 { 8 } else { 18 };
    draw_fit(
        target,
        TextRole::Title,
        Point::new(0, title_y),
        layout.width,
        "RETINUE",
        layout.value_font,
        theme.accent,
        None,
    )?;
    let mut board = Text::<40>::empty();
    let _ = write!(
        &mut board,
        "{} / {}",
        value_or_dash(&local.board),
        value_or_dash(&local.firmware)
    );
    draw_fit(
        target,
        TextRole::Line,
        Point::new(0, title_y + layout.label_to_value + 12),
        layout.width,
        board.as_str(),
        layout.label_font,
        theme.foreground,
        None,
    )?;
    draw_fit(
        target,
        TextRole::Line,
        Point::new(0, title_y + layout.label_to_value + 24),
        layout.width,
        "DISPLAY OK / RADIO ...",
        layout.label_font,
        theme.muted,
        None,
    )
}

pub(super) fn render_menu<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    selected: MenuItem,
    selected_index: u8,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
{
    header(target, layout, theme, "MENU", "LOCAL")?;
    let (items, len) = menu_items(host);
    let rows = layout.menu_rows.min(len);
    let first = if selected_index < rows {
        0
    } else {
        selected_index + 1 - rows
    };

    for visible_row in 0..rows {
        let index = first + visible_row;
        let item = items[usize::from(index)];
        let y = layout.body_y + i32::from(visible_row) * layout.menu_step;
        let is_selected = item == selected && index == selected_index;
        if is_selected {
            target.fill(
                Rectangle::new(
                    Point::new(0, y - 1),
                    Size::new(layout.width as u32, layout.menu_step as u32),
                ),
                theme.foreground,
            )?;
        }
        draw_fit(
            target,
            if is_selected {
                TextRole::Selected
            } else {
                TextRole::Line
            },
            Point::new(2, y),
            layout.width - 4,
            menu_label(item),
            layout.label_font,
            if is_selected {
                theme.background
            } else {
                theme.foreground
            },
            None,
        )?;
    }
    ticker(target, layout, theme, "MOVE / SELECT / BACK")
}

pub(super) fn render_verify<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
{
    header(target, layout, theme, "VERIFY", "HOST")?;
    let Some(node) = host.and_then(HostSnapshot::named_node) else {
        return centered(target, layout, theme, "FINGERPRINT --");
    };
    let mut first = Text::<24>::empty();
    let mut second = Text::<24>::empty();
    for (index, byte) in node.fingerprint.iter().enumerate() {
        let line = if index < 8 { &mut first } else { &mut second };
        if index % 4 == 0 && index % 8 != 0 {
            let _ = line.write_str(" ");
        }
        let _ = write!(line, "{byte:02X}");
    }
    let step = if layout.height == 64 { 14 } else { 24 };
    draw_fit(
        target,
        TextRole::Line,
        Point::new(2, layout.body_y + 2),
        layout.width - 4,
        first.as_str(),
        layout.list_font,
        theme.foreground,
        None,
    )?;
    draw_fit(
        target,
        TextRole::Line,
        Point::new(2, layout.body_y + 2 + step),
        layout.width - 4,
        second.as_str(),
        layout.list_font,
        theme.foreground,
        None,
    )?;
    ticker(target, layout, theme, "COMPARE IN PERSON")
}

pub(super) fn render_fault<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
    local: &LocalStatus,
) -> Result<(), P::Error>
where
    P: Painter,
{
    let code = local.fault.map(|fault| fault.code).unwrap_or(0);
    let mut right = Text::<8>::empty();
    let _ = write!(&mut right, "E{code:02}");
    header(target, layout, theme, "FAULT", right.as_str())?;
    let message = local
        .fault
        .as_ref()
        .map(|fault| fault.message.as_str())
        .unwrap_or("UNKNOWN");
    let banner_height = if layout.height == 64 { 17_i32 } else { 28_i32 };
    target.fill(
        Rectangle::new(
            Point::new(0, layout.body_y),
            Size::new(layout.width as u32, banner_height as u32),
        ),
        theme.foreground,
    )?;
    draw_fit(
        target,
        TextRole::Notice,
        Point::new(2, layout.body_y + 3),
        layout.width - 4,
        message,
        layout.label_font,
        theme.background,
        Some(theme.foreground),
    )?;
    draw_fit(
        target,
        TextRole::Line,
        Point::new(2, layout.body_y + banner_height + 4),
        layout.width - 4,
        "SEE HOST LOG",
        layout.label_font,
        theme.foreground,
        None,
    )?;
    ticker(target, layout, theme, "LOCAL RADIO FAULT")
}

pub(super) fn render_display_off<P>(
    target: &mut P,
    layout: &Layout,
    theme: Theme<P::Color>,
) -> Result<(), P::Error>
where
    P: Painter,
{
    header(target, layout, theme, "DISPLAY OFF", "LOCAL")?;
    centered(target, layout, theme, "KEY TO WAKE")?;
    ticker(target, layout, theme, "CPU SLEEP IS SEPARATE")
}
