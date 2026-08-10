//! Fulfilling a page's request.
//!
//! This is the only place the desktop face reaches hardware or advances the
//! owner flow, and it does both by asking the owning layer. It never plans, it
//! never decides compatibility, and it cannot execute: `begin_install` hands
//! back the plan Linkboy approved, and the worker runs *that*.

use signalman::observe_device;

use crate::state::{DesktopState, Request};
use crate::worker::Worker;

/// Perform one request. Every failure ends as visible text on the page that
/// asked, never as a silently disabled control.
pub fn perform(state: &mut DesktopState, request: Request, worker: &mut Worker) {
    match request {
        Request::Rescan => {
            state.adopt_survey(crate::survey::devices());
        }
        Request::ConfirmDevice => confirm_device(state),
        Request::ConfirmFirmware => confirm_firmware(state),
        Request::ApproveChanges => {
            if let Err(error) = state.installer.approve_changes() {
                state.refuse(&error);
            }
        }
        Request::BeginInstall => begin_install(state, worker),
    }
}

fn confirm_device(state: &mut DesktopState) {
    let Some(device) = state.device().cloned() else {
        state.refuse_with(vec!["Choose a device first.".into()]);
        return;
    };
    // The owner names the board revision. Linkboy refuses a plan without one,
    // and it is right to: a revision is a claim only a person looking at the
    // board can make.
    let revision = state.board_revision.text().trim().to_string();
    if revision.is_empty() {
        state.refuse_with(vec![
            "Enter the exact board revision printed on the board.".into(),
            "Linkboy refuses to plan a flash without it, because nothing it can \
             read off the wire tells it which revision this is."
                .into(),
        ]);
        return;
    }
    let Some(family) = family_of(device.board.as_deref()) else {
        state.refuse_with(vec![format!(
            "{} did not answer as a board this build can flash.",
            device.port
        )]);
        return;
    };
    match observe_device(&device.port, Some((family, revision))) {
        Ok(observation) => match state.installer.choose_device(observation) {
            Ok(()) => state.refusal.clear(),
            Err(error) => state.refuse(&error),
        },
        Err(error) => state.refuse_with(vec![error.to_string()]),
    }
}

fn confirm_firmware(state: &mut DesktopState) {
    let Some(catalog) = state.catalog.as_ref() else {
        state.refuse_with(vec![
            state
                .catalog_error
                .clone()
                .unwrap_or_else(|| "No verified package catalog is loaded.".into()),
        ]);
        return;
    };
    let Some(package) = state.package().map(|p| p.package_id.clone()) else {
        state.refuse_with(vec!["Choose a firmware package first.".into()]);
        return;
    };
    let loaded = match catalog.load_package(&package) {
        Ok(loaded) => loaded,
        Err(error) => {
            state.refuse_with(vec![error.to_string()]);
            return;
        }
    };
    // This is where compatibility is decided, by Linkboy, and where a refusal
    // becomes something the owner reads rather than a button that will not
    // press.
    match state.installer.choose_firmware(loaded) {
        Ok(()) => state.refusal.clear(),
        Err(error) => state.refuse(&error),
    }
}

fn begin_install(state: &mut DesktopState, worker: &mut Worker) {
    if state.install_running {
        return;
    }
    // `begin_install` is the flow's own gate: it moves the stage and hands back
    // the exact approved inputs. The face copies them to the worker and does
    // not otherwise touch them.
    let started = match state.installer.begin_install() {
        Ok((plan, package)) => worker.start(plan.clone(), package.clone()),
        Err(error) => {
            state.refuse(&error);
            return;
        }
    };
    match started {
        Ok(()) => {
            state.install_running = true;
            state.progress = Some(0.0);
            state.refusal.clear();
        }
        Err(why) => state.refuse_with(vec![format!("Could not start the installer: {why}")]),
    }
}

/// Which board family a survey line names, or `None` for one this build cannot
/// flash. Deliberately narrow: an unrecognized banner is not a guess.
fn family_of(board: Option<&str>) -> Option<linkboy::BoardFamily> {
    match board? {
        "HeltecV4" => Some(linkboy::BoardFamily::HeltecV4),
        "T114" => Some(linkboy::BoardFamily::T114),
        _ => None,
    }
}
