//! The worker bridge.
//!
//! Flashing blocks: it runs helper processes, waits out a re-enumeration, and
//! writes for tens of seconds. Doing that on the UI thread would freeze the
//! window through exactly the part an owner most needs to watch.
//!
//! So the blocking Linkboy executor runs on a dedicated thread and sends
//! structured [`FlashEvent`]s back. The `GenetAppRunner`, the DOM, and the view
//! callbacks never leave the UI thread — the channel carries owned data, not
//! anything borrowed from the tree.
//!
//! This is Signalman application code, not Cambium infrastructure. The host has
//! no idea it exists; it just gets a `frame` hook that drains a channel.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::JoinHandle;

use linkboy::executor::DEFAULT_PATIENCE;
use linkboy::{
    ExecutionError, FlashEvent, FlashPackage, FlashPlan, LiveDeviceRunner, SystemProcessRunner,
    execute_plan,
};

/// What the worker sends home.
pub enum FromWorker {
    /// One executor event, verbatim.
    Event(FlashEvent),
    /// The run ended without a terminal event of its own — an error the
    /// executor reported by returning rather than by emitting.
    Failed(String),
    /// The thread finished. Always last.
    Finished,
}

/// The UI-thread half of the bridge.
#[derive(Default)]
pub struct Worker {
    channel: Option<Receiver<FromWorker>>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `plan` against `package` on a dedicated thread.
    ///
    /// Both are owned copies of what the flow approved. The face cannot alter
    /// them on the way here — `FlashPlan`'s fields are private and its only
    /// constructor is Linkboy's planner — so what executes is what was
    /// reviewed.
    pub fn start(&mut self, plan: FlashPlan, package: FlashPackage) -> Result<(), String> {
        if self.handle.as_ref().is_some_and(|h| !h.is_finished()) {
            return Err("an install is already running".into());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("signalman-installer".into())
            .spawn(move || {
                let mut process = SystemProcessRunner;
                let mut device = LiveDeviceRunner;
                let emit_tx = tx.clone();
                let mut emit = move |event: FlashEvent| {
                    // A closed channel means the window went away; the transfer
                    // still has to finish rather than be abandoned mid-write.
                    let _ = emit_tx.send(FromWorker::Event(event));
                };
                let result = execute_plan(
                    &plan,
                    &package,
                    &mut process,
                    &mut device,
                    DEFAULT_PATIENCE,
                    &mut emit,
                );
                if let Err(error) = result {
                    // `RecoveryRequired` already emitted its own terminal event,
                    // so only the other errors need reporting here — otherwise
                    // the page would show the same failure twice.
                    if !matches!(error, ExecutionError::RecoveryRequired { .. }) {
                        let _ = tx.send(FromWorker::Failed(error.to_string()));
                    }
                }
                let _ = tx.send(FromWorker::Finished);
            })
            .map_err(|e| e.to_string())?;
        self.channel = Some(rx);
        self.handle = Some(handle);
        Ok(())
    }

    /// Drain everything the worker has sent since the last frame. Never blocks:
    /// a frame that finds nothing simply renders what it had.
    pub fn drain(&mut self) -> Vec<FromWorker> {
        let mut out = Vec::new();
        let Some(rx) = self.channel.as_ref() else {
            return out;
        };
        loop {
            match rx.try_recv() {
                Ok(message) => {
                    let finished = matches!(message, FromWorker::Finished);
                    out.push(message);
                    if finished {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // The thread went away without saying goodbye.
                    out.push(FromWorker::Finished);
                    break;
                }
            }
        }
        if out
            .iter()
            .any(|m| matches!(m, FromWorker::Finished))
        {
            self.channel = None;
        }
        out
    }

    /// Whether a run is in flight.
    pub fn running(&self) -> bool {
        self.channel.is_some()
    }
}
