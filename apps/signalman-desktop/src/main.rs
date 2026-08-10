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
use signalman_desktop::worker::{FromWorker, Worker};
use signalman_desktop::{SHEET, default_catalog_path, flow, root, survey};

type Ctx<'a> = AppCtx<'a, DesktopState, Logic, Child>;

fn main() {
    // The worker lives beside the host, not inside it: the host knows nothing
    // about threads, and this is application code.
    let worker = Rc::new(RefCell::new(Worker::new()));
    let frame_worker = worker.clone();
    let dispatch_worker = worker.clone();

    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        // Drain the installer thread. Returning `true` keeps frames coming
        // while it is running, so progress moves without polling when idle.
        frame: Box::new(move |ctx: &mut Ctx<'_>| {
            let mut worker = frame_worker.borrow_mut();
            let messages = worker.drain();
            if messages.is_empty() {
                return worker.running();
            }
            ctx.runner.update(|state| {
                for message in &messages {
                    match message {
                        FromWorker::Event(event) => state.apply_event(event),
                        FromWorker::Failed(why) => state.worker_lost(why),
                        FromWorker::Finished => {}
                    }
                }
            });
            worker.running()
        }),
        // A page asked for something that touches hardware or the flow. It runs
        // here rather than in the handler because a device survey opens serial
        // ports, and a view must stay a pure function of state.
        after_dispatch: Box::new(move |ctx: &mut Ctx<'_>| {
            let mut worker = dispatch_worker.borrow_mut();
            let mut close = false;
            ctx.runner.update(|state| {
                if let Some(request) = state.take_request() {
                    flow::perform(state, request, &mut worker);
                }
                close = state.close_requested;
            });
            if close {
                *ctx.close = true;
            }
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
        |_window| {
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
