//! `wasm-bindgen` exports. Logic lives in the native modules; this file only
//! translates names and errors.

use alloc::{string::String, vec::Vec};

use radio_face::decode_snapshot;
use wasm_bindgen::{Clamped, prelude::*};

use crate::{mirror, names};

fn js<E: core::fmt::Display>(error: E) -> JsError {
    JsError::new(&alloc::format!("{error}"))
}

fn theme_from(
    palette: Option<Vec<u32>>,
    surface: radio_face::Surface,
) -> Result<radio_face::Theme<embedded_graphics::pixelcolor::Rgb888>, JsError> {
    match palette.as_deref() {
        None => Ok(mirror::mono_theme()),
        Some([background, foreground, muted, accent]) => Ok(mirror::theme_from_rgb(
            *background,
            *foreground,
            *muted,
            *accent,
        )),
        Some([]) => Ok(mirror::receipt_theme(surface)),
        Some(_) => Err(JsError::new(
            "palette is [background, foreground, muted, accent] as 0xRRGGBB",
        )),
    }
}

#[cfg(feature = "json")]
fn host_from(json: Option<String>) -> Result<Option<radio_face::HostSnapshot>, JsError> {
    json.map(|json| crate::input::host_from_json(&json).map_err(js))
        .transpose()
}

/// One simulated radio.
#[wasm_bindgen]
pub struct RadioMirror {
    inner: mirror::Mirror,
}

#[wasm_bindgen]
impl RadioMirror {
    /// `surface`: `oled-128x64` | `tft-240x135`; `input`: `one-button` | `two-button`.
    #[wasm_bindgen(constructor)]
    pub fn new(surface: &str, input: &str) -> Result<RadioMirror, JsError> {
        Ok(Self {
            inner: mirror::Mirror::new(
                names::surface(surface).map_err(js)?,
                names::input_profile(input).map_err(js)?,
            ),
        })
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.inner.surface().size().width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.inner.surface().size().height
    }

    pub fn set_input(&mut self, input: &str) -> Result<(), JsError> {
        self.inner
            .set_profile(names::input_profile(input).map_err(js)?);
        Ok(())
    }

    /// `[background, foreground, muted, accent]` as `0xRRGGBB`; `[]` is the
    /// receipt palette; omitted is the boards' mono palette.
    pub fn set_palette(&mut self, palette: Option<Vec<u32>>) -> Result<(), JsError> {
        self.inner
            .set_theme(theme_from(palette, self.inner.surface())?);
        Ok(())
    }

    #[cfg(feature = "json")]
    pub fn set_local_json(&mut self, json: &str) -> Result<(), JsError> {
        self.inner
            .set_local(crate::input::local_from_json(json).map_err(js)?);
        Ok(())
    }

    /// A host JSON document, or `undefined` to detach.
    #[cfg(feature = "json")]
    pub fn set_host_json(&mut self, json: Option<String>) -> Result<(), JsError> {
        self.inner.set_host(host_from(json)?);
        Ok(())
    }

    /// Host snapshot bytes exactly as a host sends them to the radio.
    pub fn set_host_wire(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        let host = decode_snapshot(bytes).map_err(|error| js(alloc::format!("{error:?}")))?;
        self.inner.set_host(Some(host));
        Ok(())
    }

    pub fn age_host(&mut self, elapsed_secs: u32) -> bool {
        self.inner.age_host(elapsed_secs)
    }

    /// `a-short` | `a-long` | `b-short` | `b-long` | `chord`. Returns the action name.
    pub fn press(&mut self, event: &str) -> Result<String, JsError> {
        let event = names::input_event(event).map_err(js)?;
        Ok(names::action_name(self.inner.press(event)))
    }

    /// A raw edge (`a` | `b`). Returns `"<event> <action>"` when a press completes.
    pub fn edge(
        &mut self,
        button: &str,
        pressed: bool,
        now_ms: u32,
    ) -> Result<Option<String>, JsError> {
        let button = names::button(button).map_err(js)?;
        Ok(self
            .inner
            .edge(button, pressed, now_ms)
            .map(|(event, action)| {
                alloc::format!(
                    "{} {}",
                    names::input_event_name(event),
                    names::action_name(action)
                )
            }))
    }

    /// `status` … `peers`, `verify`, `fault`, `display-off`, or `menu:<item>:<index>`.
    pub fn screen(&self) -> String {
        names::screen_name(self.inner.screen())
    }

    #[wasm_bindgen(getter)]
    pub fn brightness(&self) -> u8 {
        self.inner.controller().brightness()
    }

    #[wasm_bindgen(getter)]
    pub fn panel_lit(&self) -> bool {
        self.inner.panel_lit()
    }

    /// `idle` | `activity` | `operation` → `off` | `double-pulse` | `slow-pulse` | `fault-triple`.
    pub fn led(&self, signal: &str) -> Result<String, JsError> {
        Ok(names::led_intent_name(self.inner.led(names::led_signal(signal).map_err(js)?)).into())
    }

    /// The current screen as RGBA, for `new ImageData(rgba, width, height)`.
    pub fn rgba(&mut self) -> Clamped<Vec<u8>> {
        Clamped(self.inner.render().to_vec())
    }

    /// A named screen with this radio's state, e.g. `boot`.
    pub fn rgba_for(&mut self, screen: &str) -> Result<Clamped<Vec<u8>>, JsError> {
        let screen = names::screen(screen).map_err(js)?;
        Ok(Clamped(self.inner.render_screen(screen).to_vec()))
    }

    /// Renders the current screen and puts it into `context` at (`x`, `y`),
    /// one canvas pixel per panel pixel. Feature `canvas`.
    #[cfg(feature = "canvas")]
    pub fn draw(
        &mut self,
        context: &web_sys::CanvasRenderingContext2d,
        x: f64,
        y: f64,
    ) -> Result<(), JsValue> {
        let size = self.inner.surface().size();
        let image = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(self.inner.render()),
            size.width,
            size.height,
        )?;
        context.put_image_data(&image, x, y)
    }

    /// What the current screen says, one line per row (`\n`-separated), for an
    /// `aria-live` region or `alt`. The rows are the ones the pixels draw.
    pub fn text(&self) -> String {
        self.inner.text().join("\n")
    }

    /// What a named screen says with this radio's state.
    pub fn text_for(&self, screen: &str) -> Result<String, JsError> {
        let screen = names::screen(screen).map_err(js)?;
        Ok(self.inner.text_for(screen).join("\n"))
    }
}

/// Stateless render of one screen from JSON documents.
#[cfg(feature = "json")]
#[wasm_bindgen]
pub fn render_screen(
    surface: &str,
    screen: &str,
    local_json: &str,
    host_json: Option<String>,
    palette: Option<Vec<u32>>,
) -> Result<Clamped<Vec<u8>>, JsError> {
    let surface = names::surface(surface).map_err(js)?;
    let local = crate::input::local_from_json(local_json).map_err(js)?;
    let host = host_from(host_json)?;
    let frame = mirror::render_rgba(
        surface,
        theme_from(palette, surface)?,
        names::screen(screen).map_err(js)?,
        &local,
        host.as_ref(),
    );
    Ok(Clamped(frame.into_rgba()))
}

/// Stateless text of one screen from JSON documents: the alt text for
/// [`render_screen`]'s pixels.
#[cfg(feature = "json")]
#[wasm_bindgen]
pub fn screen_text(
    surface: &str,
    screen: &str,
    local_json: &str,
    host_json: Option<String>,
) -> Result<String, JsError> {
    let surface = names::surface(surface).map_err(js)?;
    let local = crate::input::local_from_json(local_json).map_err(js)?;
    let host = host_from(host_json)?;
    let screen = names::screen(screen).map_err(js)?;
    Ok(radio_face::render_lines(surface, screen, &local, host.as_ref()).join("\n"))
}
