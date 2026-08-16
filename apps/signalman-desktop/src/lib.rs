#![forbid(unsafe_code)]

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

pub mod device_mere;
pub mod flow;
pub mod network;
pub mod state;
pub mod survey;
pub mod theme;
pub mod views;
pub mod worker;

pub use state::{
    DesktopSection, DesktopState, LabelDensity, ManagementSettings, NetworkRequest, Request,
    SurveyState,
};
pub use theme::{SHEET, sheet};
pub use views::{Child, Logic, root};

/// The text seam: the device-route fields, when one has focus.
///
/// The host owns the caret, selection, IME, drag selection, and visual caret
/// movement; it cannot know where an application keeps its text. This is the
/// application's half — an ancestor tags each editable field, so the host can
/// reach the corresponding state without treating every input as a revision.
pub fn focused_revision_field(
    runner: &cambium_genet_winit_host::Runner<DesktopState, Logic, Child>,
) -> Option<cambium_genet_winit_host::FocusedTextSlot<DesktopState>> {
    use layout_dom_api::LayoutDom as _;
    let node = runner.focus()?;
    let dom = runner.dom();
    let field = {
        let dom = dom.borrow();
        if !dom
            .element_name(node)
            .is_some_and(|name| name.local.as_ref() == "input")
        {
            return None;
        }
        let mut cursor = Some(node);
        let mut field = None;
        while let Some(current) = cursor {
            field = dom
                .attributes(current)
                .find(|attribute| attribute.name.local.as_ref() == "data-text-field")
                .map(|attribute| attribute.value.to_string())
                .or(field);
            if field.is_some() {
                break;
            }
            cursor = dom.parent(current);
        }
        field
    };
    match field.as_deref() {
        Some("revision") => Some(cambium_genet_winit_host::FocusedTextSlot {
            node,
            get: Box::new(|s: &DesktopState| &s.board_revision),
            get_mut: Box::new(|s: &mut DesktopState| &mut s.board_revision),
        }),
        Some("uf2-volume") => Some(cambium_genet_winit_host::FocusedTextSlot {
            node,
            get: Box::new(|s: &DesktopState| &s.t114_uf2_volume),
            get_mut: Box::new(|s: &mut DesktopState| &mut s.t114_uf2_volume),
        }),
        Some("loader-record") => Some(cambium_genet_winit_host::FocusedTextSlot {
            node,
            get: Box::new(|s: &DesktopState| &s.t114_loader_record),
            get_mut: Box::new(|s: &mut DesktopState| &mut s.t114_loader_record),
        }),
        _ => None,
    }
}

/// Where the packaged firmware catalog lives.
///
/// An installed application carries `firmware/packages` beside its executable.
/// `SIGNALMAN_CATALOG_PATH` is an explicit staging override. Falling back to
/// this checkout's package index keeps developer builds usable, but is never a
/// public-install requirement.
pub fn default_catalog_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("SIGNALMAN_CATALOG_PATH") {
        return std::path::PathBuf::from(path);
    }
    if let Some(path) = installed_catalog_path() {
        return path;
    }
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../firmware/packages/index.toml")
}

fn installed_catalog_path() -> Option<std::path::PathBuf> {
    let path = std::env::current_exe()
        .ok()?
        .parent()?
        .join("firmware/packages/index.toml");
    path.is_file().then_some(path)
}
