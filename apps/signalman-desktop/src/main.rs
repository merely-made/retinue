#![forbid(unsafe_code)]

//! The window.
//!
//! Wiring only. `cambium-genet-winit-host` owns the winit lifecycle, the
//! surface, layout, paint, hit testing, input routing, and the accessibility
//! tree; this supplies the state, the views, the sheet, and four small hooks.

use std::cell::RefCell;
use std::rc::Rc;

use cambium_genet_winit_host::{AppCtx, HostHooks, HostOptions, Init, run};
use signalman_desktop::state::DesktopState;
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::worker::Worker;
use signalman_desktop::{SHEET, default_catalog_path, flow, root, survey};

type Ctx<'a> = AppCtx<'a, DesktopState, Logic, Child>;

fn main() {
    // The worker lives beside the host, not inside it: the host knows nothing
    // about threads, and this is application code.
    let worker = Rc::new(RefCell::new(None::<Worker>));
    let wake_worker = worker.clone();
    let dispatch_worker = worker.clone();

    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        // Installation progress is event-driven: a Signalman worker calls the
        // host's Armillary-shaped wake callback, and the host grants this UI
        // thread a drain turn. Idle apps therefore stay asleep.
        frame: Box::new(|_| false),
        after_wake: Box::new(move |ctx: &mut Ctx<'_>| {
            let messages = {
                let mut slot = wake_worker.borrow_mut();
                let Some(install) = slot.as_mut() else {
                    return;
                };
                let messages = install.drain();
                if !install.running() {
                    *slot = None;
                }
                messages
            };
            if messages.is_empty() {
                return;
            }
            ctx.runner.update(|state| {
                for message in messages {
                    state.apply_install_update(message);
                }
            });
        }),
        // A page asked for something that touches hardware or the flow. It runs
        // here rather than in the handler because a device survey opens serial
        // ports, and a view must stay a pure function of state.
        after_dispatch: Box::new(move |ctx: &mut Ctx<'_>| {
            let mut worker = dispatch_worker.borrow_mut();
            let wake = ctx.wake.callback();
            ctx.runner.update(|state| {
                if let Some(request) = state.take_request() {
                    flow::perform(state, request, &mut worker, wake.clone());
                }
            });
        }),
        // A running firmware transfer needs its process, device, and recovery
        // observation to reach a terminal receipt. The native close button and
        // any future app close command therefore share this refusal.
        close_request: Box::new(|ctx: &mut Ctx<'_>, _| {
            let mut disposition = None;
            ctx.runner.update(|state| {
                disposition = Some(state.close_disposition());
            });
            disposition.expect("the runner updates close disposition")
        }),
        after_frame: Box::new(|_ctx| {}),
        focused_text: Box::new(signalman_desktop::focused_revision_field),
        key_intercept: Box::new(|_runner, _press| false),
    };

    let options = HostOptions {
        title: "Signalman — install firmware".into(),
        initial_logical_size: (960.0, 680.0),
        size_env: Some(("SIGNALMAN_WIDTH".into(), "SIGNALMAN_HEIGHT".into())),
        ..Default::default()
    };
    run(
        options,
        |_window, _commands, _wake| {
            let mut state = DesktopState::new(&default_catalog_path());
            // The first survey happens before the first frame, so the device
            // page opens with what is actually plugged in rather than with a
            // spinner that resolves a moment later.
            state.adopt_survey(survey::devices());
            Init {
                state,
                logic: root as Logic,
                sheet: SHEET.to_string(),
            }
        },
        hooks,
    )
    .expect("run app");
}
