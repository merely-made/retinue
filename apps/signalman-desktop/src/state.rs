//! The desktop face's own state.
//!
//! Everything here is presentation and selection. Package trust, compatibility,
//! plans, execution, and recovery decisions stay behind Signalman and Linkboy —
//! this type cannot construct, alter, or execute a `FlashPlan`, and the only
//! way it advances the owner flow is by calling the flow's own methods.
//!
//! It is deliberately free of I/O. Device surveys and the executor run
//! elsewhere and hand their results in, which is what lets the whole six-page
//! flow be driven in a headless test with no board plugged in.

use linkboy::executor::{ExecutionStage, RecoveryFacts};
use linkboy::package::RecoveryInstructions;
use linkboy::{FlashEvent, FlashReceipt, OwnerStage, ReceiptResult};
use signalman::{
    DeviceCandidate, FirmwareCatalog, FirmwareInstaller, FirmwareView, describe_event,
    event_progress, refusal_lines,
};

/// A side-effecting step the view asks for and the application loop performs.
///
/// Views never touch a serial port or start a thread: a handler records the
/// intent, and the loop that owns the hardware fulfils it. That is also why a
/// test can drive every page without a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Re-survey the machine's ports.
    Rescan,
    /// Take the selected port into the flow.
    ConfirmDevice,
    /// Take the selected package into the flow (this is where a refusal comes
    /// from).
    ConfirmFirmware,
    /// The owner approved the reviewed plan.
    ApproveChanges,
    /// Hand the approved plan to the worker.
    BeginInstall,
}

/// How the survey went, so an empty list can say which kind of empty it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurveyState {
    /// No survey has run yet.
    #[default]
    Unasked,
    /// A survey ran; `devices` is what it found (possibly nothing).
    Surveyed,
}

pub struct DesktopState {
    /// The owner flow. The only thing that can move a page.
    pub installer: FirmwareInstaller,
    /// The verified package catalog, or why it could not be loaded.
    pub catalog: Option<FirmwareCatalog>,
    pub catalog_error: Option<String>,

    pub devices: Vec<DeviceCandidate>,
    pub survey: SurveyState,
    pub selected_device: Option<usize>,
    pub selected_package: Option<usize>,
    /// The board revision the owner types. A plan is refused without it, and
    /// that refusal is shown rather than hidden behind a disabled control.
    pub board_revision: cambium::TextInput,

    /// The current refusal, as separate visible lines. Cleared when the owner
    /// changes something that could resolve it.
    pub refusal: Vec<String>,
    /// The execution event log, oldest first.
    pub notes: Vec<String>,
    /// Transfer progress, `0.0..=1.0`, while one is running.
    pub progress: Option<f32>,
    /// The last execution stage a recovery reported.
    pub recovery: Option<RecoveryFacts>,
    pub recovery_instructions: Option<RecoveryInstructions>,
    /// Set once the plan has been handed to the worker, so it is handed over
    /// once and the Install page can say it is running.
    pub install_running: bool,

    /// What the view asked the application loop to do.
    pub pending: Option<Request>,
    /// Set by the close control.
    pub close_requested: bool,
}

impl DesktopState {
    /// A fresh flow with a catalog loaded from `index_path`. A catalog that
    /// will not verify is a visible state, not a panic: the first page says so
    /// and the flow simply cannot leave the firmware step.
    pub fn new(index_path: &std::path::Path) -> Self {
        let (catalog, catalog_error) = match FirmwareCatalog::load(index_path) {
            Ok(catalog) => (Some(catalog), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            installer: FirmwareInstaller::new(),
            catalog,
            catalog_error,
            devices: Vec::new(),
            survey: SurveyState::default(),
            selected_device: None,
            selected_package: None,
            board_revision: cambium::TextInput::default(),
            refusal: Vec::new(),
            notes: Vec::new(),
            progress: None,
            recovery: None,
            recovery_instructions: None,
            install_running: false,
            pending: None,
            close_requested: false,
        }
    }

    /// The flow's own projection: which page, and every field it carries.
    pub fn view(&self) -> FirmwareView {
        self.installer.view()
    }

    /// Which of the six pages is showing.
    pub fn stage(&self) -> OwnerStage {
        self.installer.view().stage
    }

    /// The chosen device, if one is selected and still in the list.
    pub fn device(&self) -> Option<&DeviceCandidate> {
        self.devices.get(self.selected_device?)
    }

    /// The chosen catalog package, if one is selected.
    pub fn package(&self) -> Option<&linkboy::CatalogPackage> {
        self.catalog
            .as_ref()?
            .packages()
            .get(self.selected_package?)
    }

    /// Ask the application loop for something. Views call this; nothing here
    /// performs it.
    pub fn request(&mut self, request: Request) {
        self.pending = Some(request);
    }

    /// Take whatever the view asked for.
    pub fn take_request(&mut self) -> Option<Request> {
        self.pending.take()
    }

    /// Adopt a completed device survey.
    pub fn adopt_survey(&mut self, devices: Vec<DeviceCandidate>) {
        // Keep the selection only if the same port is still there: a port index
        // that silently slid onto a different board would be the worst possible
        // kind of "it worked".
        let selected_port = self.device().map(|d| d.port.clone());
        self.devices = devices;
        self.selected_device = selected_port
            .and_then(|port| self.devices.iter().position(|d| d.port == port));
        self.survey = SurveyState::Surveyed;
        self.refusal.clear();
    }

    /// Select a device by index, clearing any refusal it might resolve.
    pub fn select_device(&mut self, index: usize) {
        if index < self.devices.len() {
            self.selected_device = Some(index);
            self.refusal.clear();
        }
    }

    /// Select a catalog package by index.
    pub fn select_package(&mut self, index: usize) {
        let count = self.catalog.as_ref().map_or(0, |c| c.packages().len());
        if index < count {
            self.selected_package = Some(index);
            self.refusal.clear();
        }
    }

    /// Record a refusal from the owning flow, as separate visible lines.
    pub fn refuse(&mut self, error: &linkboy::FlowError) {
        self.refusal = refusal_lines(error);
    }

    /// Record a refusal Signalman raised outside the flow.
    pub fn refuse_with(&mut self, lines: Vec<String>) {
        self.refusal = lines;
    }

    /// Feed one executor event into the flow and the log. The flow decides what
    /// it means for the page; this decides what the owner reads.
    pub fn apply_event(&mut self, event: &FlashEvent) {
        self.installer.apply_event(event);
        if let Some(line) = describe_event(event) {
            // A progress line replaces its predecessor rather than filling the
            // log with one entry per chunk.
            if matches!(event, FlashEvent::Writing { .. })
                && self
                    .notes
                    .last()
                    .is_some_and(|last| last.starts_with("Writing "))
            {
                self.notes.pop();
            }
            self.notes.push(line);
        }
        if let Some(progress) = event_progress(event) {
            self.progress = Some(progress);
        }
        match event {
            FlashEvent::Complete { .. } => {
                self.progress = Some(1.0);
                self.install_running = false;
                self.recovery = None;
                self.recovery_instructions = None;
            }
            FlashEvent::RecoveryRequired {
                facts,
                instructions,
                ..
            } => {
                self.install_running = false;
                self.recovery = Some(facts.clone());
                self.recovery_instructions = Some(instructions.clone());
            }
            FlashEvent::Refused { reasons } => {
                self.install_running = false;
                self.refusal = reasons.iter().map(ToString::to_string).collect();
            }
            _ => {}
        }
    }

    /// The worker died without a terminal event. Said plainly, because a
    /// transfer that stopped for an unknown reason is a recovery situation and
    /// not a success.
    pub fn worker_lost(&mut self, why: &str) {
        self.install_running = false;
        self.notes.push(format!("The installer stopped: {why}"));
    }

    /// Whether the flow reached a finished receipt.
    pub fn completed(&self) -> bool {
        matches!(self.view().result, Some(ReceiptResult::Complete))
    }

    /// Whether the flow is in recovery.
    pub fn needs_recovery(&self) -> bool {
        matches!(self.view().result, Some(ReceiptResult::RecoveryRequired))
            || self.recovery.is_some()
    }

    /// The receipt, once one exists.
    pub fn receipt(&self) -> Option<FlashReceipt> {
        self.installer.receipt().cloned()
    }

    /// The stage a recovery stopped at, in owner words.
    pub fn recovery_stage(&self) -> Option<&'static str> {
        Some(match self.recovery.as_ref()?.stage {
            ExecutionStage::Preparing => "while preparing",
            ExecutionStage::EnteringBootloader => "while entering the bootloader",
            ExecutionStage::Transfer => "during the transfer",
            ExecutionStage::Rebooting => "while rebooting",
            ExecutionStage::VerifyingApplication => "while verifying the application",
        })
    }
}
