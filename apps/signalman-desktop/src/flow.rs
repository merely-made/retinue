//! Fulfilling a page's request.
//!
//! This is the only place the desktop face reaches hardware or advances the
//! owner flow, and it does both by asking the owning layer. It never plans,
//! decides compatibility, or runs helpers: Signalman starts the exact approved
//! install and owns its blocking worker.

use signalman::{
    InstallerWake, capture_t114_uf2_volume,
    observe_device_with_board_selection_and_t114_loader_snapshot, observe_t114_serial_dfu_port,
};

use crate::state::{DesktopState, Request};
use crate::worker::Worker;

/// Perform one request. Every failure ends as visible text on the page that
/// asked, never as a silently disabled control.
pub fn perform(
    state: &mut DesktopState,
    request: Request,
    worker: &mut Option<Worker>,
    wake: InstallerWake,
) {
    match request {
        Request::Rescan => {
            state.adopt_survey(crate::survey::devices());
        }
        Request::ConfirmDevice => confirm_device(state),
        Request::ConfirmMountedT114 => confirm_mounted_t114(state),
        Request::ConfirmT114Dfu => confirm_t114_dfu(state),
        Request::ConfirmFirmware => confirm_firmware(state),
        Request::ApproveChanges => {
            if let Err(error) = state.installer.approve_changes() {
                state.refuse(&error);
            }
        }
        Request::BeginInstall => begin_install(state, worker, wake),
    }
}

fn confirm_device(state: &mut DesktopState) {
    let Some(device) = state.device().cloned() else {
        state.refuse_with(vec!["Choose a device first.".into()]);
        return;
    };
    // The owner supplies a carrier marking or intentionally chooses a documented product
    // profile. Linkboy refuses a plan without a revision source; it must never infer one from a
    // serial port or USB identity.
    let revision = state.board_revision.text().trim().to_string();
    if revision.is_empty() {
        state.refuse_with(vec![
            "Enter the exact board revision or choose a documented product profile.".into(),
            "Linkboy refuses to plan a flash without a source, because nothing it can \
             read off the wire tells it which carrier revision this is."
                .into(),
        ]);
        return;
    }
    let Some(family) = family_of(device.board.as_deref()).or(state.selected_board_family.clone())
    else {
        state.refuse_with(vec![format!(
            "{} did not answer as a board this build can flash.",
            device.port
        )]);
        return;
    };
    let selection = state.board_selection(family.clone(), revision);
    let loader_snapshot = if family == linkboy::BoardFamily::T114
        && !state.t114_loader_record.text().trim().is_empty()
    {
        match linkboy::T114LoaderSnapshot::from_json(state.t114_loader_record.text().trim()) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                state.refuse_with(vec![error.to_string()]);
                return;
            }
        }
    } else {
        None
    };
    if family == linkboy::BoardFamily::T114 && device.board.is_none() && loader_snapshot.is_none() {
        state.refuse_with(vec![
            "A silent T114 needs its captured HT-n5262 UF2 loader record before Linkboy can plan a serial DFU restore.".into(),
            "Enter the loader-record path created while the board was mounted, then try again.".into(),
        ]);
        return;
    }
    match observe_device_with_board_selection_and_t114_loader_snapshot(
        &device.port,
        Some(selection),
        loader_snapshot.as_ref(),
    ) {
        Ok(observation) => match state.installer.choose_device(observation) {
            Ok(()) => state.refusal.clear(),
            Err(error) => state.refuse(&error),
        },
        Err(error) => state.refuse_with(vec![error.to_string()]),
    }
}

fn confirm_mounted_t114(state: &mut DesktopState) {
    let volume = state.t114_uf2_volume.text().trim().to_string();
    let record_path = state.t114_loader_record.text().trim().to_string();
    let revision = state.board_revision.text().trim().to_string();
    if volume.is_empty() {
        state.refuse_with(vec![
            "Enter the mounted T114 UF2 volume, for example D:\\.".into(),
        ]);
        return;
    }
    if revision.is_empty() {
        state.refuse_with(vec!["Enter the exact T114 board revision first.".into()]);
        return;
    }
    if record_path.is_empty() {
        state.refuse_with(vec![
            "Choose where to retain the T114 loader record before installing foreign firmware."
                .into(),
            "The later serial-DFU restore needs the board's own UF2 and SoftDevice facts.".into(),
        ]);
        return;
    }
    match capture_t114_uf2_volume(&volume, revision, &record_path) {
        Ok(observation) => match state.installer.choose_device(observation) {
            Ok(()) => state.refusal.clear(),
            Err(error) => state.refuse(&error),
        },
        Err(error) => state.refuse_with(vec![error.to_string()]),
    }
}

fn confirm_t114_dfu(state: &mut DesktopState) {
    let Some(device) = state.device().cloned() else {
        state.refuse_with(vec!["Choose the T114 DFU port first.".into()]);
        return;
    };
    if device.board.is_some() {
        state.refuse_with(vec![
            "This port answered as an application; use the ordinary device action instead.".into(),
        ]);
        return;
    }
    if state.selected_board_family.as_ref() != Some(&linkboy::BoardFamily::T114) {
        state.refuse_with(vec![
            "Declare the selected silent serial device as the T114 you own first.".into(),
        ]);
        return;
    }
    let revision = state.board_revision.text().trim().to_string();
    if revision.is_empty() {
        state.refuse_with(vec!["Enter the exact T114 board revision first.".into()]);
        return;
    }
    let record_path = state.t114_loader_record.text().trim();
    if record_path.is_empty() {
        state.refuse_with(vec![
            "Enter the loader-record path captured from this T114 first.".into(),
        ]);
        return;
    }
    let snapshot = match linkboy::T114LoaderSnapshot::from_json(record_path) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            state.refuse_with(vec![error.to_string()]);
            return;
        }
    };
    let observation = observe_t114_serial_dfu_port(&device.port, revision, &snapshot);
    match state.installer.choose_device(observation) {
        Ok(()) => state.refusal.clear(),
        Err(error) => state.refuse(&error),
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

fn begin_install(state: &mut DesktopState, worker: &mut Option<Worker>, wake: InstallerWake) {
    if state.install_running {
        return;
    }
    // `start_install` is Signalman's own gate: it moves the owner flow and
    // starts the exact approved inputs. The face contributes only its host's
    // wake callback, so a completed worker can be drained on the UI thread.
    let started = match state.installer.start_install(wake) {
        Ok(worker) => worker,
        Err(error) => {
            state.refuse_with(vec![error.to_string()]);
            return;
        }
    };
    *worker = Some(started);
    state.install_running = true;
    state.progress = Some(0.0);
    state.refusal.clear();
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
