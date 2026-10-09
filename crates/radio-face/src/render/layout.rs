//! Panel sizes, colour themes and the per-surface layout grid.

use embedded_graphics::{
    mono_font::{
        MonoFont,
        ascii::{FONT_4X6, FONT_5X7, FONT_6X10, FONT_9X15},
    },
    pixelcolor::PixelColor,
    prelude::*,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Oled128x64,
    Tft240x135,
}

impl Surface {
    pub const fn size(self) -> Size {
        match self {
            Self::Oled128x64 => Size::new(128, 64),
            Self::Tft240x135 => Size::new(240, 135),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme<C: PixelColor> {
    pub background: C,
    pub foreground: C,
    pub muted: C,
    pub accent: C,
}

impl<C: PixelColor> Theme<C> {
    pub const fn new(background: C, foreground: C, muted: C, accent: C) -> Self {
        Self {
            background,
            foreground,
            muted,
            accent,
        }
    }
}

pub(super) struct Layout {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) header_divider_y: i32,
    pub(super) body_y: i32,
    pub(super) second_row_y: i32,
    pub(super) ticker_divider_y: i32,
    pub(super) ticker_y: i32,
    pub(super) column_width: i32,
    pub(super) header_font: &'static MonoFont<'static>,
    pub(super) label_font: &'static MonoFont<'static>,
    pub(super) value_font: &'static MonoFont<'static>,
    pub(super) ticker_font: &'static MonoFont<'static>,
    pub(super) list_font: &'static MonoFont<'static>,
    pub(super) label_to_value: i32,
    pub(super) list_step: i32,
    pub(super) menu_step: i32,
    pub(super) menu_rows: u8,
}

impl Layout {
    pub(super) fn new(surface: Surface) -> Self {
        match surface {
            Surface::Oled128x64 => Self {
                width: 128,
                height: 64,
                header_divider_y: 9,
                body_y: 12,
                second_row_y: 34,
                ticker_divider_y: 54,
                ticker_y: 57,
                column_width: 64,
                header_font: &FONT_5X7,
                label_font: &FONT_4X6,
                value_font: &FONT_6X10,
                ticker_font: &FONT_4X6,
                list_font: &FONT_5X7,
                label_to_value: 7,
                list_step: 13,
                menu_step: 10,
                menu_rows: 4,
            },
            Surface::Tft240x135 => Self {
                width: 240,
                height: 135,
                header_divider_y: 17,
                body_y: 22,
                second_row_y: 67,
                ticker_divider_y: 118,
                ticker_y: 123,
                column_width: 120,
                header_font: &FONT_6X10,
                label_font: &FONT_6X10,
                value_font: &FONT_9X15,
                ticker_font: &FONT_6X10,
                list_font: &FONT_9X15,
                label_to_value: 12,
                list_step: 27,
                menu_step: 19,
                menu_rows: 5,
            },
        }
    }
}
