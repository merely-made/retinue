//! The drawing calls a screen is made of, and their pixel implementation.

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyleBuilder},
    pixelcolor::PixelColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text as EgText},
};

/// What a run of drawn text is on its screen, for the text projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    /// A screen's title, or the boot banner.
    Title,
    /// The header's right-hand state, e.g. `RAD OK`.
    Status,
    /// A field's label; its value follows.
    Label,
    Value,
    /// A list, menu or body line.
    Line,
    /// The highlighted menu item.
    Selected,
    /// A centred message or the fault banner.
    Notice,
    /// The bottom ticker.
    Ticker,
}

/// The drawing calls a screen is made of. Pixels and the text projection are
/// two implementations, so they cannot disagree about what a page shows.
pub(crate) trait Painter {
    type Color: PixelColor + Copy;
    type Error;

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error>;
    /// `visible` is already clipped to the space it is drawn in.
    fn text(
        &mut self,
        role: TextRole,
        position: Point,
        visible: &str,
        font: &'static MonoFont<'static>,
        color: Self::Color,
        background: Option<Self::Color>,
    ) -> Result<(), Self::Error>;
    fn fill(&mut self, area: Rectangle, color: Self::Color) -> Result<(), Self::Error>;
    /// A one-pixel line.
    fn line(&mut self, line: Line, color: Self::Color) -> Result<(), Self::Error>;
}

/// Paints onto any `DrawTarget`.
pub(super) struct Pixels<'a, D>(pub(super) &'a mut D);

impl<D> Painter for Pixels<'_, D>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    type Color = D::Color;
    type Error = D::Error;

    fn clear(&mut self, color: D::Color) -> Result<(), D::Error> {
        self.0.clear(color)
    }

    fn text(
        &mut self,
        _role: TextRole,
        position: Point,
        visible: &str,
        font: &'static MonoFont<'static>,
        color: D::Color,
        background: Option<D::Color>,
    ) -> Result<(), D::Error> {
        let builder = MonoTextStyleBuilder::new().font(font).text_color(color);
        let style = if let Some(background) = background {
            builder.background_color(background).build()
        } else {
            builder.build()
        };
        EgText::with_baseline(visible, position, style, Baseline::Top).draw(self.0)?;
        Ok(())
    }

    fn fill(&mut self, area: Rectangle, color: D::Color) -> Result<(), D::Error> {
        area.into_styled(PrimitiveStyle::with_fill(color))
            .draw(self.0)
    }

    fn line(&mut self, line: Line, color: D::Color) -> Result<(), D::Error> {
        line.into_styled(PrimitiveStyle::with_stroke(color, 1))
            .draw(self.0)
    }
}
