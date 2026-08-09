//! Signalman's desktop face for the owner firmware flow.
//!
//! Six pages over Linkboy's owner flow, projected through Signalman, rendered
//! by Cambium on genet's single-root desktop host. The layer rules the scope
//! sets are enforced by what this crate can reach:
//!
//! - it depends on `signalman` for vocabulary, never on Linkboy policy;
//! - it cannot construct, alter, or execute a `FlashPlan` — `FlashPlan`'s
//!   fields are private, its only constructor is Linkboy's planner, and the
//!   only way to obtain one here is `FirmwareInstaller::begin_install`, which
//!   is the flow's own gate;
//! - the blocking executor runs on a worker thread and sends structured events
//!   home; the runner, the DOM, and the view callbacks stay on the UI thread.
//!
//! The library half exists so the headless page-state and keyboard tests can
//! drive the same state machine and the same views the binary runs.

pub mod flow;
pub mod state;
pub mod theme;
pub mod views;
pub mod worker;

pub use state::{DesktopState, Request, SurveyState};
pub use theme::SHEET;
pub use views::{Child, Logic, root};

/// The text seam: the board-revision field, when it has focus.
///
/// The host owns the caret, selection, IME, drag selection, and visual caret
/// movement; it cannot know where an application keeps its text. This is the
/// application's half — there is exactly one editable field in the whole flow,
/// so recognizing the focused `<input>` is the whole job.
pub fn focused_revision_field(
    runner: &cambium_genet_winit_host::Runner<DesktopState, Logic, Child>,
) -> Option<cambium_genet_winit_host::FocusedTextSlot<DesktopState>> {
    use layout_dom_api::LayoutDom as _;
    let node = runner.focus()?;
    let dom = runner.dom();
    let is_input = {
        let dom = dom.borrow();
        dom.element_name(node)
            .is_some_and(|name| name.local.as_ref() == "input")
    };
    if !is_input {
        return None;
    }
    Some(cambium_genet_winit_host::FocusedTextSlot {
        node,
        get: Box::new(|s: &DesktopState| &s.board_revision),
        get_mut: Box::new(|s: &mut DesktopState| &mut s.board_revision),
    })
}

/// Where the packaged firmware catalog lives, relative to this crate.
///
/// A shipped build will resolve this from an installed data directory; for now
/// it is the repository's own package index, which is what the physical
/// acceptance runs flash from.
pub fn default_catalog_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../firmware/packages/index.toml")
}
