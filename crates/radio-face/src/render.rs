//! Screen drawing for the supported panels.

use embedded_graphics::{pixelcolor::PixelColor, prelude::*};

use crate::{
    controller::Screen,
    status::{HostSnapshot, LocalStatus},
};

mod labels;
mod layout;
mod pages;
mod painter;
mod screens;
#[cfg(test)]
mod tests;
mod widgets;

use layout::Layout;
pub use layout::{Surface, Theme};
use pages::render_page;
pub(crate) use painter::Painter;
use painter::Pixels;
pub use painter::TextRole;
use screens::{render_boot, render_display_off, render_fault, render_menu, render_verify};

pub fn render<D>(
    target: &mut D,
    surface: Surface,
    theme: Theme<D::Color>,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), D::Error>
where
    D: DrawTarget,
    D::Color: PixelColor + Copy,
{
    paint(&mut Pixels(target), surface, theme, screen, local, host)
}

/// Every screen is painted through [`Painter`], so the pixels and the text
/// projection come from the same calls.
pub(crate) fn paint<P>(
    target: &mut P,
    surface: Surface,
    theme: Theme<P::Color>,
    screen: Screen,
    local: &LocalStatus,
    host: Option<&HostSnapshot>,
) -> Result<(), P::Error>
where
    P: Painter,
{
    let layout = Layout::new(surface);
    target.clear(theme.background)?;

    match screen {
        Screen::Boot => render_boot(target, &layout, theme, local),
        Screen::Page(page) => render_page(target, &layout, theme, page, local, host),
        Screen::Menu {
            selected,
            selected_index,
        } => render_menu(target, &layout, theme, selected, selected_index, host),
        Screen::Verify => render_verify(target, &layout, theme, host),
        Screen::Fault => render_fault(target, &layout, theme, local),
        Screen::DisplayOff => render_display_off(target, &layout, theme),
    }
}
