//! Mounted appearance regressions over the production view and host seams.

use cambium_genet_winit_host::{Harness, HostHooks, Init};
use genet_probe::Selector;
use signalman_desktop::{
    DesktopState,
    appearance::{AppearanceState, PRODUCT_DEFAULT},
    default_catalog_path, root,
    views::{Child, Logic},
};
use tabard::{
    Theme,
    library::ThemeLibraryStore,
    theme::{
        choice::{FileThemeChoiceStore, ThemeChoice, ThemeChoiceStore},
        registry::{Mode, ThemeRegistry},
    },
};
use tempfile::tempdir;

type Host = Harness<DesktopState, Logic, Child>;

fn saved_theme(library: &std::path::Path) -> Theme {
    let seeds = ThemeRegistry::default().list()[0].seeds;
    let mut theme = Theme::new("theme:signalman-authored", "Signalman authored", seeds);
    theme.mode_sheets.insert(
        "custom:garden".into(),
        vec![":root { --tabard-color-bg: #203020; --tabard-color-text: #f1f2f3; }".into()],
    );
    theme.mode_sheets.insert(
        "dark".into(),
        vec![":root { --tabard-color-bg: #123456; --tabard-color-text: #f1f2f3; }".into()],
    );
    ThemeLibraryStore::load(library)
        .unwrap()
        .save(&[theme.clone()])
        .unwrap();
    theme
}

fn launch(selection: &std::path::Path, library: &std::path::Path) -> Host {
    let mut state = DesktopState::new(&default_catalog_path());
    state.appearance =
        AppearanceState::load(selection.to_path_buf(), library.to_path_buf(), vec![]);
    let sheet = state.appearance.take_stylesheet_change().unwrap();
    let mut previews = signalman_desktop::appearance_host::PreviewBindings::default();
    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        frame: Box::new(move |ctx| {
            previews.frame(ctx);
            false
        }),
        after_dispatch: Box::new(|ctx| {
            signalman_desktop::appearance_host::after_dispatch_with_exporter(ctx, |_| None)
        }),
        focused_text: Box::new(signalman_desktop::focused_revision_field),
        close_request: Box::new(|ctx, _| {
            let mut disposition = None;
            ctx.runner
                .update(|state| disposition = Some(state.close_disposition()));
            disposition.unwrap()
        }),
        ..HostHooks::inert()
    };
    let mut host = Harness::with_hooks(
        Init {
            state,
            logic: root as Logic,
            sheet,
            fonts: vec![],
            images: vec![],
        },
        hooks,
    );
    host.layout_at(1280.0, 960.0);
    host
}

fn click(host: &mut Host, selector: Selector) {
    assert!(host.click_on(&selector), "mounted control {selector:?}");
    host.after_dispatch();
    host.relayout();
}
fn action(host: &mut Host, key: &str) {
    click(host, Selector::role("button").with_attr("data-action", key));
}
fn background(host: &Host) -> String {
    let node = host.with_dom(|dom| genet_probe::matching(dom, &Selector::class("app-shell"))[0]);
    host.computed_value(node, "background-color")
        .unwrap()
        .to_ascii_lowercase()
        .replace(' ', "")
}

#[test]
fn mounted_modes_and_authored_css_preserve_domain_state_and_reopen_exact_choice() {
    let dir = tempdir().unwrap();
    let selection = dir.path().join("appearance.json");
    let library = dir.path().join("themes.json");
    let theme = saved_theme(&library);
    let mut host = launch(&selection, &library);
    let original = (
        host.state().section,
        host.state().management_settings,
        host.state().network_epoch,
        host.state().network_pan,
        host.state().network_zoom,
        host.state().selected_device,
        host.state().install_running,
    );
    assert_eq!(host.state().appearance.active_id(), PRODUCT_DEFAULT);
    assert!(!selection.exists());
    action(&mut host, "toggle-appearance");
    click(
        &mut host,
        Selector::role("button").with_attr("data-appearance-theme", "theme:default"),
    );
    let mut colors = std::collections::BTreeSet::new();
    for mode in [Mode::Light, Mode::Dark, Mode::HcLight, Mode::HcDark] {
        click(
            &mut host,
            Selector::role("button").with_attr("data-appearance-mode", &mode.as_key()),
        );
        assert_eq!(
            host.state()
                .appearance
                .applied()
                .unwrap()
                .resolved
                .theme_mode,
            Some(mode)
        );
        colors.insert(background(&host));
    }
    assert_eq!(
        colors.len(),
        4,
        "the mounted product must use all four actual palettes"
    );
    click(
        &mut host,
        Selector::role("button").with_attr("data-appearance-theme", &theme.id),
    );
    click(
        &mut host,
        Selector::role("button").with_attr("data-appearance-mode", "dark"),
    );
    assert!(matches!(
        background(&host).as_str(),
        "#123456" | "rgb(18,52,86)" | "rgba(18,52,86,1)"
    ));
    click(
        &mut host,
        Selector::role("button").with_attr("data-appearance-mode", "custom:garden"),
    );
    assert!(matches!(
        background(&host).as_str(),
        "#203020" | "rgb(32,48,32)" | "rgba(32,48,32,1)"
    ));
    assert_eq!(
        original,
        (
            host.state().section,
            host.state().management_settings,
            host.state().network_epoch,
            host.state().network_pan,
            host.state().network_zoom,
            host.state().selected_device,
            host.state().install_running
        )
    );
    let reopened = launch(&selection, &library);
    assert_eq!(
        host.state().appearance.applied(),
        reopened.state().appearance.applied()
    );
    assert_eq!(background(&host), background(&reopened));
    click(
        &mut host,
        Selector::role("button").with_attr("data-appearance-theme", PRODUCT_DEFAULT),
    );
    assert!(host.state().appearance.applied().is_none());
    assert_eq!(
        host.state().appearance.stylesheet(),
        launch(&dir.path().join("fresh.json"), &library)
            .state()
            .appearance
            .stylesheet()
    );
}

#[test]
fn corrupt_preferences_and_library_are_visible_and_preserved() {
    let dir = tempdir().unwrap();
    let selection = dir.path().join("appearance.json");
    let library = dir.path().join("themes.json");
    std::fs::write(&selection, b"malformed preferences").unwrap();
    std::fs::write(&library, b"malformed library").unwrap();
    let mut state = AppearanceState::load(selection.clone(), library.clone(), vec![]);
    assert!(!state.authoring_available);
    assert!(
        state
            .diagnostics()
            .iter()
            .any(|notice| notice.contains("existing file is preserved"))
    );
    assert!(
        state
            .request_select("theme:default", Some(Mode::Light))
            .is_err()
    );
    state.commit_requested();
    assert!(state.applied().is_none());
    state.notice = Some("Unrelated action completed".into());
    assert!(
        state
            .diagnostics()
            .iter()
            .any(|notice| notice.contains("existing file is preserved"))
    );
    assert!(
        state
            .diagnostics()
            .iter()
            .any(|notice| notice.contains("authoring is unavailable"))
    );
    assert_eq!(std::fs::read(selection).unwrap(), b"malformed preferences");
    assert_eq!(std::fs::read(&library).unwrap(), b"malformed library");
    let mut valid_choice = AppearanceState::load(
        dir.path().join("valid-choice.json"),
        library.clone(),
        vec![],
    );
    valid_choice
        .request_select("theme:default", Some(Mode::Dark))
        .unwrap();
    valid_choice.commit_requested();
    assert!(valid_choice.applied().is_some());
    assert!(
        valid_choice
            .diagnostics()
            .iter()
            .any(|notice| notice.contains("authoring is unavailable"))
    );
    assert_eq!(std::fs::read(library).unwrap(), b"malformed library");
}

#[test]
fn failed_preference_write_does_not_publish_the_requested_presentation() {
    let dir = tempdir().unwrap();
    let selection = dir.path().join("appearance.json");
    let library = dir.path().join("themes.json");
    let mut state = AppearanceState::load(selection.clone(), library, vec![]);
    state.select("theme:default", Some(Mode::Light)).unwrap();
    let original = state.applied().cloned();
    let sheet = state.stylesheet();
    std::fs::remove_file(&selection).unwrap();
    std::fs::create_dir(&selection).unwrap();
    std::fs::write(selection.join("preserved"), b"owned settings destination").unwrap();
    state
        .request_select("theme:default", Some(Mode::Dark))
        .unwrap();
    assert_eq!(
        state.applied(),
        original.as_ref(),
        "views only request the effect"
    );
    state.commit_requested();
    assert_eq!(state.applied(), original.as_ref());
    assert_eq!(state.stylesheet(), sheet);
    assert!(state.notice.unwrap().contains("not saved"));
    assert_eq!(
        std::fs::read(selection.join("preserved")).unwrap(),
        b"owned settings destination"
    );
}

#[test]
fn saving_active_definition_requires_explicit_apply_and_preserves_installer_close_refusal() {
    let dir = tempdir().unwrap();
    let selection = dir.path().join("appearance.json");
    let library = dir.path().join("themes.json");
    let theme = saved_theme(&library);
    let mut state = DesktopState::new(&default_catalog_path());
    state.appearance = AppearanceState::load(selection.clone(), library.clone(), vec![]);
    state
        .appearance
        .select(&theme.id, Some(Mode::Dark))
        .unwrap();
    let original = state.appearance.applied().cloned();
    state.appearance.begin_edit().unwrap();
    *state
        .appearance
        .workshop
        .text_field_mut("mode-sheet")
        .unwrap() = cambium::TextInput::new(":root { --tabard-color-bg: #abcdef; }");
    state.appearance.workshop.apply_stylesheet();
    state.appearance.workshop.save();
    state.appearance.workshop.saved_choice().unwrap();
    assert_eq!(state.appearance.applied(), original.as_ref());
    state.appearance.apply_workshop().unwrap();
    assert_eq!(state.appearance.applied(), original.as_ref());
    state.appearance.commit_requested();
    assert_ne!(state.appearance.applied(), original.as_ref());
    let reopened = AppearanceState::load(selection, library, vec![]);
    assert_eq!(state.appearance.applied(), reopened.applied());
    state.install_running = true;
    assert_eq!(
        state.close_disposition(),
        cambium_genet_winit_host::CloseDisposition::KeepVisible
    );
    assert!(!state.appearance.close_app);
    assert!(
        state
            .refusal
            .iter()
            .any(|line| line.contains("Installation is still active"))
    );
}

#[test]
fn missing_definition_fallback_keeps_the_saved_request_and_bytes() {
    let dir = tempdir().unwrap();
    let selection = dir.path().join("appearance.json");
    let choice = ThemeChoice::new("theme:later", Some(Mode::HcLight));
    FileThemeChoiceStore::load_strict(&selection)
        .unwrap()
        .set_choice(choice.clone())
        .unwrap();
    let bytes = std::fs::read(&selection).unwrap();
    let state = AppearanceState::load(selection.clone(), dir.path().join("themes.json"), vec![]);
    assert_eq!(state.applied().unwrap().requested, choice);
    assert!(
        state
            .diagnostics()
            .iter()
            .any(|notice| notice.contains("unavailable"))
    );
    assert_eq!(std::fs::read(selection).unwrap(), bytes);
}

#[test]
fn application_preferences_and_owned_files_are_protected_from_theme_export() {
    let dir = tempdir().unwrap();
    let selection = dir.path().join("appearance.json");
    let library = dir.path().join("themes.json");
    let capture = dir.path().join("capture.json");
    let missing = dir.path().join("future-capture.json");
    std::fs::write(&capture, b"owned observation capture").unwrap();
    let mut state = AppearanceState::load(
        selection.clone(),
        library,
        vec![capture.clone(), missing.clone()],
    );
    state.select("theme:default", Some(Mode::Dark)).unwrap();
    let captures = dir.path().join("captures");
    let generation = captures.join("capture-2000-1.json");
    state.protect_owned_exports(
        vec![capture.clone(), missing.clone()],
        vec![captures.clone()],
    );
    let before = std::fs::read(&selection).unwrap();
    for path in [&selection, &capture, &missing, &generation] {
        state.workshop.request_export();
        let artifact = state.workshop.take_export().unwrap();
        state.workshop.complete_export(
            artifact,
            Some(
                path.parent()
                    .unwrap()
                    .join(".")
                    .join(path.file_name().unwrap()),
            ),
        );
        assert!(
            state
                .workshop
                .status()
                .contains("protected application files")
        );
        assert!(state.workshop.replacement_path().is_none());
    }
    assert_eq!(std::fs::read(&selection).unwrap(), before);
    assert_eq!(
        std::fs::read(&capture).unwrap(),
        b"owned observation capture"
    );
    assert!(!missing.exists());
    assert!(!generation.exists());
    assert!(
        !captures.exists(),
        "protected future generations cannot create the data directory"
    );
}

#[test]
fn dirty_editor_close_cancel_and_save_use_the_shared_guard() {
    let dir = tempdir().unwrap();
    let mut state = DesktopState::new(&default_catalog_path());
    state.appearance = AppearanceState::load(
        dir.path().join("appearance.json"),
        dir.path().join("themes.json"),
        vec![],
    );
    state.appearance.begin_edit().unwrap();
    *state.appearance.workshop.text_field_mut("name").unwrap() =
        cambium::TextInput::new("Signalman guarded draft");
    assert_eq!(
        state.close_disposition(),
        cambium_genet_winit_host::CloseDisposition::KeepVisible
    );
    assert!(state.appearance.workshop.close_requested());
    state.appearance.workshop.cancel_close();
    assert!(!signalman_desktop::appearance_view::sync_editor(&mut state));
    assert!(state.appearance.editor_open);
    assert!(!state.appearance.close_app);
    assert_eq!(
        state.appearance.workshop.draft_theme().name,
        "Signalman guarded draft"
    );
    assert_eq!(
        state.close_disposition(),
        cambium_genet_winit_host::CloseDisposition::KeepVisible
    );
    state.appearance.workshop.save_and_close();
    assert!(signalman_desktop::appearance_view::sync_editor(&mut state));
    assert!(!state.appearance.editor_open);
    assert!(dir.path().join("themes.json").exists());
    assert!(
        state.appearance.applied().is_none(),
        "save and close never implicitly applies a definition"
    );
}

#[test]
fn appearance_scenarios_parse_through_the_shared_lane() {
    for source in [
        include_str!("../scenarios/appearance.scn"),
        include_str!("../scenarios/appearance_reopen.scn"),
    ] {
        genet_probe::Scenario::parse(source).unwrap();
    }
}
