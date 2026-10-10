//! Device-free appearance acceptance through the shared native scenario lane.

use cambium::{Key, KeyEvent, NamedKey};
use genet_probe::ProbeSnapshot;
use layout_dom_api::{LayoutDom, LocalName, Namespace};

use crate::{
    DesktopState,
    appearance_host::Context,
    views::{Child, Logic},
};

pub struct Product {
    sheet: String,
    authority: Option<String>,
}

fn authority(state: &DesktopState) -> String {
    format!(
        "{:?}|{:?}|{:?}|{}|{}|{:?}|{:?}|{:?}|{:?}|{}|{}",
        state.section,
        state.stage(),
        state.management_settings,
        state.network_epoch,
        state.install_running,
        state.network_pan,
        state.network_zoom,
        state.selected_device,
        state.selected_package,
        state.observation_collecting,
        state.devices.len()
    )
}

fn focus_name(ctx: &Context<'_>) -> String {
    let Some(node) = ctx.runner.focus() else {
        return "none".into();
    };
    let dom = ctx.runner.dom();
    let dom = dom.borrow();
    for name in [
        "data-action",
        "data-field",
        "data-mode",
        "data-appearance-mode",
        "data-appearance-theme",
        "aria-label",
    ] {
        if let Some(value) = dom.attribute(node, &Namespace::from(""), &LocalName::from(name)) {
            return format!("{name}={value}");
        }
    }
    "other".into()
}

pub fn from_env() -> Option<mesquite::Lane<Product>> {
    let config = mesquite::LaneConfig::from_env("SIGNALMAN_APPEARANCE")?;
    Some(
        mesquite::Lane::from_config(
            config,
            Product {
                sheet: crate::theme::sheet(),
                authority: None,
            },
            cambium_genet_winit_host::read_file,
        )
        .expect("load Signalman appearance scenario")
        .with_frame_limit(Some(1800)),
    )
}

pub fn drive(lane: &mut mesquite::Lane<Product>, ctx: &mut Context<'_>) {
    let product = lane.product_mut();
    product.sheet = ctx.runner.state().appearance.stylesheet();
    if product.authority.is_none() {
        product.authority = Some(authority(ctx.runner.state()));
    }
    lane.after_frame(ctx);
}

impl mesquite::Product for Product {
    type State = DesktopState;
    type Logic = Logic;
    type View = Child;
    const KIND: &'static str = "signalman-appearance";
    const SURFACE: &'static str = "app";
    const LOG_PREFIX: &'static str = "signalman-appearance";
    fn sheet(&self) -> &str {
        &self.sheet
    }
    fn busy_mut(&mut self, _: &mut Context<'_>, pending: bool) -> Option<bool> {
        Some(pending)
    }
    fn snapshot(&self, ctx: &Context<'_>, _: usize, _: f32) -> ProbeSnapshot {
        let state = ctx.runner.state();
        let appearance = &state.appearance;
        ProbeSnapshot::default()
            .with_field("theme", appearance.active_id())
            .with_field(
                "mode",
                appearance
                    .applied()
                    .and_then(|resolved| resolved.resolved.theme_mode.as_ref())
                    .map_or_else(|| "product_default".into(), |mode| mode.as_key()),
            )
            .with_field("editor", appearance.editor_open.to_string())
            .with_field("name", appearance.workshop.draft_theme().name.clone())
            .with_field("dirty", appearance.workshop.is_dirty().to_string())
            .with_field("preview_mode", appearance.workshop.mode_key())
            .with_field(
                "saved",
                appearance.workshop.saved_choice().is_ok().to_string(),
            )
            .with_field(
                "authority_preserved",
                (self.authority.as_ref() == Some(&authority(state))).to_string(),
            )
            .with_field("focus", focus_name(ctx))
    }
    fn app_step(
        &mut self,
        ctx: &mut Context<'_>,
        _: mesquite::Checkpoints<'_>,
        line: &str,
    ) -> Result<(), String> {
        if let Some(target) = line.strip_prefix("key tab-until ") {
            for _ in 0..200 {
                if focus_name(ctx) == target.trim() {
                    return Ok(());
                }
                ctx.runner
                    .dispatch_key(KeyEvent::new(Key::Named(NamedKey::Tab)));
            }
            return Err(format!("Tab never reached {target}"));
        }
        Err(format!("unknown appearance scenario verb: {line}"))
    }
    fn complete(&mut self, ctx: &mut Context<'_>, _: &genet_probe::Outcome) -> Result<(), String> {
        if self.authority.as_ref() != Some(&authority(ctx.runner.state())) {
            return Err("Appearance changed Signalman's domain authority".into());
        }
        Ok(())
    }
}
