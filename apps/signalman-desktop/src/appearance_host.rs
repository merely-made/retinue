//! Native seams for the embedded shared editor, over Signalman's existing host.

use crate::{
    DesktopState,
    views::{Child, Logic},
};
use cambium_genet_winit_host::{AppCtx, choose_save_path};
use tabard_workshop::{
    ExportFormat,
    native_host::{PreviewBindings as WorkshopPreviews, sync_and_export},
};

pub type Context<'a> = AppCtx<'a, DesktopState, Logic, Child>;

#[derive(Default)]
pub struct PreviewBindings {
    workshop: WorkshopPreviews,
}

impl PreviewBindings {
    pub fn frame(&mut self, ctx: &mut Context<'_>) {
        if ctx.runner.state().appearance.editor_open {
            self.workshop.register(
                &ctx.runner.state().appearance.workshop,
                ctx.leaves,
                ctx.producers,
            );
        }
    }
}

pub fn after_dispatch(ctx: &mut Context<'_>) {
    after_dispatch_with_exporter(ctx, |artifact| {
        choose_save_path(
            "Export theme",
            &artifact.suggested_name,
            &[if artifact.format == ExportFormat::Css {
                "css"
            } else {
                "json"
            }],
        )
    });
}

pub fn after_dispatch_with_exporter(
    ctx: &mut Context<'_>,
    mut destination: impl FnMut(&tabard_workshop::ExportArtifact) -> Option<std::path::PathBuf>,
) {
    let mut close = false;
    ctx.runner.update(|state| {
        state.appearance.commit_requested();
        if state.appearance.editor_open {
            // Protect the current application owners before the shared helper
            // processes an explicit export and its destination/replacement.
            protect_owned_exports(state);
            sync_and_export(&mut state.appearance.workshop, &mut destination);
            // Signalman decides whether a workshop exit closes this embedded
            // surface or completes a pending application-close request.
            close = crate::appearance_view::sync_editor(state);
        }
    });
    let mut sheet = None;
    ctx.runner
        .update(|state| sheet = state.appearance.take_stylesheet_change());
    if let Some(sheet) = sheet {
        *ctx.set_sheet = Some(sheet);
    }
    if close {
        *ctx.close = true;
    }
}

pub fn load(state: &mut DesktopState) {
    use directories::{BaseDirs, ProjectDirs};
    let selection = std::env::var_os("SIGNALMAN_APPEARANCE_STORE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            ProjectDirs::from("made", "mere", "signalman")
                .map(|dirs| dirs.config_dir().join("appearance.json"))
        });
    let library = std::env::var_os("SIGNALMAN_THEME_LIBRARY")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            BaseDirs::new().map(|dirs| dirs.data_local_dir().join("mere/tabard/themes.json"))
        });
    match (selection, library) {
        (Some(selection), Some(library)) => {
            let mut protected = vec![crate::default_availability_settings_path(), crate::default_message_store_path(), crate::default_catalog_path()];
            if let Some(paths) = std::env::var_os("SIGNALMAN_OBSERVATION_FIXTURES") { protected.extend(std::env::split_paths(&paths)); }
            state.appearance = crate::appearance::AppearanceState::load(selection, library, protected);
            protect_owned_exports(state);
        }
        _ => state.appearance.notice = Some("Appearance is available for this session. Durable preferences and authoring are unavailable because the application storage directory could not be found.".into()),
    }
}

fn protect_owned_exports(state: &mut DesktopState) {
    let mut paths = vec![
        crate::default_availability_settings_path(),
        crate::default_message_store_path(),
        crate::default_catalog_path(),
    ];
    if let Some(fixtures) = std::env::var_os("SIGNALMAN_OBSERVATION_FIXTURES") {
        paths.extend(std::env::split_paths(&fixtures));
    }
    let observation = state.observation_load_path.text();
    if !observation.trim().is_empty() {
        paths.push(observation.into());
    }
    state
        .appearance
        .protect_owned_exports(paths, vec![crate::default_observation_capture_dir()]);
}
