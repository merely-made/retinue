#![forbid(unsafe_code)]

//! The window.
//!
//! Wiring only. `cambium-genet-winit-host` owns the winit lifecycle, the
//! surface, layout, paint, hit testing, input routing, and the accessibility
//! tree; this supplies the state, the views, the sheet, and four small hooks.

use std::cell::RefCell;
use std::rc::Rc;

use cambium_genet_winit_host::{AppCtx, HostHooks, HostOptions, Init, run};
use signalman::management::ManagementPresence;
use signalman_desktop::network::{LayoutWake, NETWORK_LEAF_KEY, NetworkWorker};
use signalman_desktop::state::DesktopState;
use signalman_desktop::state::NetworkRequest;
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::worker::Worker;
use signalman_desktop::{default_catalog_path, flow, root, sheet, survey};

type Ctx<'a> = AppCtx<'a, DesktopState, Logic, Child>;

fn perform_network_request(
    slot: &mut Option<NetworkWorker>,
    request: NetworkRequest,
    wake: LayoutWake,
) {
    let network = slot.get_or_insert_with(|| NetworkWorker::spawn(wake));
    match request {
        NetworkRequest::Reconcile(input) => {
            network.reconcile(input);
        }
        NetworkRequest::Pin(node, position) => {
            network.pin(node, position);
        }
        NetworkRequest::Unpin(node) => {
            network.unpin(node);
        }
    }
}

fn main() {
    // The worker lives beside the host, not inside it: the host knows nothing
    // about threads, and this is application code.
    let worker = Rc::new(RefCell::new(None::<Worker>));
    let wake_worker = worker.clone();
    let dispatch_worker = worker.clone();
    let network = Rc::new(RefCell::new(None::<NetworkWorker>));
    let wake_network = network.clone();
    let dispatch_network = network.clone();
    let last_leaf = Rc::new(RefCell::new(None));
    let frame_leaf = last_leaf.clone();

    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        // Installation progress is event-driven: a Signalman worker calls the
        // host's Armillary-shaped wake callback, and the host grants this UI
        // thread a drain turn. Idle apps therefore stay asleep.
        frame: Box::new(move |ctx| {
            let swatch = ctx.runner.state().network_swatch();
            let mut last = frame_leaf.borrow_mut();
            if last.as_ref() != Some(&swatch) {
                let leaf = swatch.paint_leaf(|presence| match presence {
                    ManagementPresence::Live => sprigging::ColorF {
                        r: 0.35,
                        g: 0.72,
                        b: 0.56,
                        a: 1.0,
                    },
                    ManagementPresence::Stale => sprigging::ColorF {
                        r: 0.43,
                        g: 0.46,
                        b: 0.52,
                        a: 1.0,
                    },
                });
                ctx.leaves.insert(NETWORK_LEAF_KEY, Box::new(leaf));
                *last = Some(swatch);
            }
            false
        }),
        after_wake: Box::new(move |ctx: &mut Ctx<'_>| {
            let messages = {
                let mut slot = wake_worker.borrow_mut();
                if let Some(install) = slot.as_mut() {
                    let messages = install.drain();
                    if !install.running() {
                        *slot = None;
                    }
                    messages
                } else {
                    Vec::new()
                }
            };
            let layout = wake_network
                .borrow()
                .as_ref()
                .and_then(NetworkWorker::take_latest);
            if messages.is_empty() && layout.is_none() {
                return;
            }
            let mut network_request = None;
            ctx.runner.update(|state| {
                for message in messages {
                    state.apply_install_update(message);
                }
                if let Some(layout) = layout {
                    state.adopt_network_layout(layout);
                }
                network_request = state.take_network_request();
            });
            if let Some(request) = network_request {
                perform_network_request(
                    &mut wake_network.borrow_mut(),
                    request,
                    ctx.wake.callback(),
                );
            }
        }),
        // A page asked for something that touches hardware or the flow. It runs
        // here rather than in the handler because a device survey opens serial
        // ports, and a view must stay a pure function of state.
        after_dispatch: Box::new(move |ctx: &mut Ctx<'_>| {
            let mut worker = dispatch_worker.borrow_mut();
            let wake = ctx.wake.callback();
            let mut network_request = None;
            ctx.runner.update(|state| {
                if let Some(request) = state.take_request() {
                    flow::perform(state, request, &mut worker, wake.clone());
                }
                network_request = state.take_network_request();
            });
            drop(worker);
            if let Some(request) = network_request {
                perform_network_request(&mut dispatch_network.borrow_mut(), request, wake);
            }
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
        title: "Signalman".into(),
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
                sheet: sheet(),
            }
        },
        hooks,
    )
    .expect("run app");
}
