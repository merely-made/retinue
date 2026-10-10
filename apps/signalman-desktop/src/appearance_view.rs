//! Reachable application controls and the existing shared workshop surface.

use cambium::{PointerClick, button, el, lens, text};
use tabard::theme::registry::THEME_ID_DEFAULT;
use tabard_workshop::workshop_view;

use crate::appearance::PRODUCT_DEFAULT;
use crate::state::DesktopState;
use crate::views::Child;

pub const EDITOR_CSS: &str = "
.signalman-editor { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.signalman-editor > .tabard-workshop { flex: 1; min-height: 0; }
.signalman-editor-controls { display: flex; align-items: center; gap: 12px; padding: 10px 18px; background: #fff; color: #253247; }
.signalman-editor-controls button { display: block; padding: 8px 12px; border: 1px solid currentColor; }
.signalman-editor-notice { display: block; padding: 8px 18px; margin: 0; background: #fff4d8; color: #493d25; }
";

pub const APPEARANCE_CSS: &str = "
.signalman-appearance { display: block; padding: 14px 24px; background: var(--tabard-color-surface, #171b23); color: var(--tabard-color-text, #e8e6e1); }
.signalman-appearance-options { display: flex; flex-wrap: wrap; gap: 8px; margin: 10px 0; }
.signalman-appearance-options button, .signalman-appearance-edit { display: block; padding: 8px 12px; border: 1px solid currentColor; background: transparent; color: inherit; }
.signalman-appearance-options button[aria-pressed=\"true\"] { background: var(--tabard-color-primary, #2f5b8c); color: var(--tabard-color-on-primary, #f2f5f9); }
.signalman-appearance-options button:focus-visible, .signalman-appearance-edit:focus-visible { box-shadow: inset 0 0 0 2px currentColor; }
.signalman-appearance-notice { display: block; line-height: 1.5; margin: 8px 0; }
.cambium-title-bar { background: var(--tabard-color-surface, #171b23); color: var(--tabard-color-text, #e8e6e1); --titlebar-min-height: 42px; }
.signalman-mark { display: block; font-weight: 700; padding: 4px 7px; border: 1px solid currentColor; }
.signalman-appearance-toggle { display: block; padding: 6px 10px; background: transparent; border: 1px solid currentColor; color: inherit; }
";

pub fn chrome(state: &DesktopState) -> Child {
    cambium::title_bar(
        Box::new(
            el("span", text("S"))
                .attr("class", "signalman-mark")
                .attr("aria-hidden", "true"),
        ),
        Box::new(el("span", text("Signalman"))),
        Box::new(
            button("Appearance", |state: &mut DesktopState, _: PointerClick| {
                state.appearance.open = !state.appearance.open;
            })
            .attr("class", "signalman-appearance-toggle")
            .attr("data-action", "toggle-appearance")
            .attr("aria-expanded", state.appearance.open.to_string()),
        ),
        Box::new(el("span", ())),
    )
}

pub fn panel(state: &DesktopState) -> Child {
    let appearance = &state.appearance;
    let mut themes: Vec<Child> = vec![Box::new(
        button(
            "Signalman default",
            |state: &mut DesktopState, _: PointerClick| {
                state.appearance.notice =
                    state.appearance.request_select(PRODUCT_DEFAULT, None).err();
            },
        )
        .attr("data-appearance-theme", PRODUCT_DEFAULT)
        .attr(
            "aria-pressed",
            (appearance.active_id() == PRODUCT_DEFAULT).to_string(),
        ),
    )];
    themes.extend(
        appearance
            .workshop
            .registry()
            .list()
            .into_iter()
            .filter(|theme| theme.id != PRODUCT_DEFAULT)
            .map(|theme| {
                let id = theme.id.clone();
                Box::new(
                    button(
                        theme.name.clone(),
                        move |state: &mut DesktopState, _: PointerClick| {
                            state.appearance.notice =
                                state.appearance.request_select(&id, None).err();
                        },
                    )
                    .attr("data-appearance-theme", theme.id.clone())
                    .attr(
                        "aria-pressed",
                        (appearance.active_id() == theme.id).to_string(),
                    ),
                ) as Child
            }),
    );
    let modes: Vec<Child> = appearance
        .modes()
        .into_iter()
        .map(|mode| {
            let key = mode.as_key();
            let active = appearance
                .applied()
                .is_some_and(|resolved| resolved.resolved.theme_mode.as_ref() == Some(&mode));
            Box::new(
                button(
                    mode.label(),
                    move |state: &mut DesktopState, _: PointerClick| {
                        let id = if state.appearance.active_id() == PRODUCT_DEFAULT {
                            THEME_ID_DEFAULT.to_owned()
                        } else {
                            state.appearance.active_id().to_owned()
                        };
                        state.appearance.notice = state
                            .appearance
                            .request_select(&id, Some(mode.clone()))
                            .err();
                    },
                )
                .attr("data-appearance-mode", key)
                .attr("aria-pressed", active.to_string()),
            ) as Child
        })
        .collect();
    let mut children: Vec<Child> = vec![
        Box::new(el("h2", text("Appearance"))),
        Box::new(
            el("div", themes)
                .attr("class", "signalman-appearance-options")
                .attr("aria-label", "Theme"),
        ),
        Box::new(
            el("div", modes)
                .attr("class", "signalman-appearance-options")
                .attr("aria-label", "Presentation mode"),
        ),
    ];
    children.extend(appearance.diagnostics().into_iter().map(|notice| {
        Box::new(
            el("p", text(notice))
                .attr("class", "signalman-appearance-notice")
                .attr("role", "status"),
        ) as Child
    }));
    if appearance.authoring_available {
        children.push(Box::new(
            button(
                "Edit themes…",
                |state: &mut DesktopState, _: PointerClick| {
                    state.appearance.notice = state.appearance.begin_edit().err();
                },
            )
            .attr("class", "signalman-appearance-edit")
            .attr("data-action", "edit-appearance"),
        ));
    }
    Box::new(
        el("section", children)
            .attr("class", "signalman-appearance")
            .attr("aria-label", "Application appearance"),
    )
}

pub fn editor(state: &DesktopState) -> Child {
    let mut children: Vec<Child> = vec![Box::new(
        el(
            "div",
            (
                button(
                    "Back to Signalman",
                    |state: &mut DesktopState, _: PointerClick| {
                        state.appearance.close_app = false;
                        if state.appearance.workshop.request_close() {
                            state.appearance.workshop.cancel_close();
                            state.appearance.editor_open = false;
                        }
                    },
                )
                .attr("data-action", "back-to-signalman"),
                button(
                    "Apply to Signalman",
                    |state: &mut DesktopState, _: PointerClick| {
                        state.appearance.notice = match state.appearance.apply_workshop() {
                            Ok(()) => None,
                            Err(error) => Some(error),
                        };
                    },
                )
                .attr("data-action", "apply-to-signalman"),
                el(
                    "p",
                    text("Preview and save your definition here, then apply it to Signalman."),
                ),
            ),
        )
        .attr("class", "signalman-editor-controls"),
    )];
    for notice in state.appearance.diagnostics() {
        children.push(Box::new(
            el("p", text(notice))
                .attr("class", "signalman-editor-notice")
                .attr("role", "status"),
        ));
    }
    children.push(Box::new(lens(
        |workshop: &mut tabard_workshop::WorkshopState| workshop_view(workshop),
        |state: &mut DesktopState| &mut state.appearance.workshop,
    )));
    Box::new(
        el("div", children)
            .attr("class", "signalman-editor")
            .attr("data-surface", "signalman.appearance-workshop.v1"),
    )
}

// Project the shared dispatch tail's exit request into Signalman's embedded
// surface and application close policy. Controls and exports have already been
// synchronized by the shared native-host helper; the lens keeps its input state.
pub fn sync_editor(state: &mut DesktopState) -> bool {
    if !state.appearance.editor_open {
        return false;
    }
    if state.appearance.workshop.exit_requested() {
        if state.appearance.workshop.has_changes() {
            state.appearance.workshop.discard();
        }
        state.appearance.workshop.cancel_close();
        state.appearance.editor_open = false;
        return std::mem::take(&mut state.appearance.close_app);
    }
    if !state.appearance.workshop.close_requested() {
        state.appearance.close_app = false;
    }
    false
}
