#![forbid(unsafe_code)]

//! The window.
//!
//! Wiring only. `cambium-genet-winit-host` owns the winit lifecycle, the
//! surface, layout, paint, hit testing, input routing, and the accessibility
//! tree; this supplies the state, the views, the sheet, and four small hooks.

use std::cell::RefCell;
use std::rc::Rc;

use cambium_genet_winit_host::{AppCtx, HostHooks, HostOptions, Init, run};
use signalman::observation::persistence::{DurableCapture, StoreOutcome};
use signalman_desktop::audio::{self, AudioEvent, AudioOperation, AudioWorker};
use signalman_desktop::availability::{
    AvailabilitySettings, accept_live_bundle, export_capture, load_capture, load_settings,
    save_settings,
};
use signalman_desktop::network::{LayoutWake, NETWORK_LEAF_KEY, NetworkWorker, paint_network_leaf};
use signalman_desktop::state::{AudioRequest, DesktopState, NetworkRequest, ObservationRequest};
use signalman_desktop::station::{self, StationWorker};
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::worker::Worker;
use signalman_desktop::{
    MessageStore, default_availability_settings_path, default_catalog_path,
    default_message_store_path, flow, root, sheet, survey,
};

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

fn perform_audio_request(
    slot: &mut Option<AudioWorker>,
    request: AudioRequest,
    wake: LayoutWake,
) -> Result<(), AudioEvent> {
    let operation = match &request {
        AudioRequest::StartCapture { .. } | AudioRequest::StopCapture => AudioOperation::Capture,
        AudioRequest::Play { .. } => AudioOperation::Playback,
    };
    if slot.is_none() {
        *slot = Some(
            AudioWorker::spawn(wake).map_err(|error| AudioEvent::Failed {
                operation,
                message: format!("could not start the host audio worker: {error}"),
            })?,
        );
    }
    let worker = slot.as_ref().expect("audio worker was installed");
    let accepted = match request {
        AudioRequest::StartCapture {
            device_id,
            max_duration,
        } => worker.start_capture(device_id, max_duration),
        AudioRequest::StopCapture => worker.stop_capture(),
        AudioRequest::Play { device_id, voice } => worker.play(device_id, voice),
    };
    if accepted {
        Ok(())
    } else {
        Err(AudioEvent::Failed {
            operation,
            message: "the host audio worker stopped before accepting the request".into(),
        })
    }
}

fn perform_observation_request(state: &mut DesktopState, request: ObservationRequest) {
    match request {
        ObservationRequest::Load => {
            let path = std::path::PathBuf::from(state.observation_load_path.text());
            match load_capture(&path) {
                Ok(capture) => state.adopt_availability(capture),
                Err(error) => state.observation_notice = Some(error),
            }
        }
        ObservationRequest::Export => {
            let Some(index) = state.selected_availability else {
                state.observation_notice = Some("Select a loaded capture before exporting.".into());
                return;
            };
            let path = std::path::PathBuf::from(state.observation_export_path.text());
            state.observation_notice =
                Some(match export_capture(&path, &state.availability[index]) {
                    Ok(()) => format!("Exported the selected capture to {}.", path.display()),
                    Err(error) => error,
                });
        }
        ObservationRequest::SaveSettings => {
            let settings = AvailabilitySettings {
                durable: state.observation_durable,
                retention_entries: state.observation_retention_entries,
                retention_bytes: state.observation_retention_bytes,
                retention_age_ms: state.observation_retention_age_ms,
            };
            if let Err(error) = save_settings(&default_availability_settings_path(), settings) {
                state.observation_notice =
                    Some(format!("Availability settings were not saved: {error}"));
            }
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
    let audio = Rc::new(RefCell::new(None::<AudioWorker>));
    let wake_audio = audio.clone();
    let dispatch_audio = audio.clone();
    let station = Rc::new(RefCell::new(None::<StationWorker>));
    let wake_station = station.clone();
    let init_station = station.clone();
    let last_leaf = Rc::new(RefCell::new(None));
    let frame_leaf = last_leaf.clone();
    let (fixture_tx, fixture_rx) = std::sync::mpsc::channel::<
        Result<signalman_desktop::availability::AvailabilityCapture, String>,
    >();
    let fixture_rx = Rc::new(RefCell::new(fixture_rx));
    let wake_fixtures = fixture_rx.clone();

    let hooks: HostHooks<DesktopState, Logic, Child> = HostHooks {
        // Installation progress is event-driven: a Signalman worker calls the
        // host's Armillary-shaped wake callback, and the host grants this UI
        // thread a drain turn. Idle apps therefore stay asleep.
        frame: Box::new(move |ctx| {
            let swatch = ctx.runner.state().network_swatch();
            let mut last = frame_leaf.borrow_mut();
            if last.as_ref() != Some(&swatch) {
                let leaf = paint_network_leaf(&swatch);
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
            let audio_events = wake_audio
                .borrow()
                .as_ref()
                .map(AudioWorker::drain)
                .unwrap_or_default();
            let station_events = wake_station
                .borrow()
                .as_ref()
                .map(StationWorker::drain)
                .unwrap_or_default();
            let fixture_events: Vec<_> = wake_fixtures.borrow().try_iter().collect();
            if messages.is_empty()
                && layout.is_none()
                && audio_events.is_empty()
                && station_events.is_empty()
                && fixture_events.is_empty()
            {
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
                for event in audio_events {
                    state.apply_audio_event(event);
                }
                for event in station_events {
                    state.apply_station_event(event);
                }
                for event in fixture_events {
                    match event {
                        Ok(capture) => {
                            let settings = AvailabilitySettings {
                                durable: state.observation_durable,
                                retention_entries: state.observation_retention_entries,
                                retention_bytes: state.observation_retention_bytes,
                                retention_age_ms: state.observation_retention_age_ms,
                            };
                            let durable_directory =
                                std::env::var_os("SIGNALMAN_OBSERVATION_DURABLE_DIR")
                                    .map(std::path::PathBuf::from);
                            let mut destination_error = None;
                            let destination = match (settings.durable, durable_directory) {
                                (true, Some(directory)) => {
                                    match std::fs::create_dir_all(&directory) {
                                        Ok(()) => DurableCapture::CreateNew(directory.join(
                                            format!(
                                                "fixture-{}-{}.json",
                                                capture.stored.captured_unix_ms,
                                                state.availability.len()
                                            ),
                                        )),
                                        Err(error) => {
                                            destination_error = Some(format!(
                                                "Durable capture directory is unavailable: {error}"
                                            ));
                                            DurableCapture::Disabled
                                        }
                                    }
                                }
                                (true, None) => {
                                    destination_error = Some(
                                        "Durable capture is on, but no capture directory is configured."
                                            .into(),
                                    );
                                    DurableCapture::Disabled
                                }
                                _ => DurableCapture::Disabled,
                            };
                            match accept_live_bundle(
                                capture.source,
                                &capture.stored.bundle,
                                capture.stored.captured_unix_ms,
                                settings,
                                &destination,
                                capture.stored.omitted_prefix_entries,
                            ) {
                                Ok((capture, outcome)) => {
                                    state.adopt_availability(capture);
                                    state.observation_notice = Some(
                                        destination_error.unwrap_or_else(|| match outcome {
                                            Ok(StoreOutcome::Disabled) => {
                                                "Live observation rendered without a host write.".into()
                                            }
                                            Ok(StoreOutcome::Written { bytes, entries }) => format!(
                                                "Durable capture wrote {entries} entries in {bytes} bytes."
                                            ),
                                            Err(error) => format!(
                                                "Live observation rendered, but durable storage failed: {error}"
                                            ),
                                        }),
                                    );
                                }
                                Err(error) => state.observation_notice = Some(error),
                            }
                        }
                        Err(error) => state.observation_notice = Some(error),
                    }
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
            let mut audio_request = None;
            let mut observation_request = None;
            ctx.runner.update(|state| {
                if let Some(request) = state.take_request() {
                    flow::perform(state, request, &mut worker, wake.clone());
                }
                network_request = state.take_network_request();
                audio_request = state.take_audio_request();
                observation_request = state.take_observation_request();
            });
            drop(worker);
            if let Some(request) = network_request {
                perform_network_request(&mut dispatch_network.borrow_mut(), request, wake.clone());
            }
            if let Some(request) = audio_request
                && let Err(event) =
                    perform_audio_request(&mut dispatch_audio.borrow_mut(), request, wake)
            {
                ctx.runner.update(|state| state.apply_audio_event(event));
            }
            if let Some(request) = observation_request {
                ctx.runner
                    .update(|state| perform_observation_request(state, request));
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
        move |_window, _commands, wake| {
            let mut state = DesktopState::new(&default_catalog_path());
            let settings_path = default_availability_settings_path();
            if settings_path.exists() {
                match load_settings(&settings_path) {
                    Ok(settings) => {
                        state.observation_durable = settings.durable;
                        state.observation_retention_entries = settings.retention_entries;
                        state.observation_retention_bytes = settings.retention_bytes;
                        state.observation_retention_age_ms = settings.retention_age_ms;
                    }
                    Err(error) => {
                        state.observation_notice = Some(format!(
                            "Availability settings were refused; safe defaults are active: {error}"
                        ));
                    }
                }
            }
            if let Some(paths) = std::env::var_os("SIGNALMAN_OBSERVATION_FIXTURES") {
                let paths: Vec<_> = std::env::split_paths(&paths).collect();
                let delay_ms = std::env::var("SIGNALMAN_OBSERVATION_FIXTURE_DELAY_MS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(1_500);
                let tx = fixture_tx.clone();
                let fixture_wake = wake.callback();
                std::thread::spawn(move || {
                    for path in paths {
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                        let loaded = load_capture(&path).map(|mut capture| {
                            capture.source = format!("fixture {}", path.display());
                            capture
                        });
                        if tx.send(loaded).is_err() {
                            break;
                        }
                        fixture_wake();
                    }
                });
                state.section = signalman_desktop::state::DesktopSection::Radio;
                let fixture_notice = "Receipt fixtures will arrive through the live wake path.";
                state.observation_notice = Some(match state.observation_notice.take() {
                    Some(previous) => format!("{previous} {fixture_notice}"),
                    None => fixture_notice.into(),
                });
            }
            // The live station is a bench activation for now: it starts only
            // when SIGNALMAN_STATION_PORT names a running Retinue board. The
            // actor's events land in after_wake like every other worker's.
            if let Some(settings) = station::settings_from_env() {
                state.station_notice = Some(format!(
                    "Connecting station \u{201c}{}\u{201d} on {}\u{2026}",
                    settings.name, settings.port
                ));
                *init_station.borrow_mut() = Some(StationWorker::spawn(settings, wake.callback()));
            }
            match MessageStore::open(default_message_store_path(), "signalman-local") {
                Ok(store) => state.replace_message_store(store),
                Err(error) => {
                    state.message_notice = Some(format!(
                        "Durable message history could not be opened: {error}"
                    ));
                }
            }
            match audio::inventory() {
                Ok(inventory) => state.adopt_audio_inventory(inventory),
                Err(error) => {
                    state.message_notice =
                        Some(format!("Host audio devices could not be listed: {error}"));
                }
            }
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
