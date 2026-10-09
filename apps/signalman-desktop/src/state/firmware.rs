//! The six-page owner firmware flow.

use linkboy::{BoardFamily, FlashEvent, FlashReceipt, OwnerStage, ReceiptResult};
use signalman::{
    DeviceCandidate, FirmwareInstallNotice, FirmwareInstallRecovery, FirmwareInstallStage,
    FirmwareInstallUpdate, FirmwareView, describe_event, event_progress, refusal_lines,
};

use super::{
    DesktopState, MESHNOLOGY_N39_DOCUMENTATION_URL, MESHNOLOGY_N39_NAME, Request, SurveyState,
    V4ProductProfile,
};

impl DesktopState {
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
        if self.observation_collecting {
            self.observation_notice = Some(
                "Stop observation collection before surveying, planning, or installing on serial devices."
                    .into(),
            );
            return;
        }
        self.pending = Some(request);
    }

    /// Take whatever the view asked for.
    pub fn take_request(&mut self) -> Option<Request> {
        self.pending.take()
    }

    /// Adopt a completed device survey.
    pub fn adopt_survey(&mut self, devices: Vec<DeviceCandidate>) {
        // Keep the selection only if the same port is still there; an index
        // must never slide onto a different board.
        let selected_port = self.device().map(|d| d.port.clone());
        self.devices = devices;
        self.selected_device = selected_port
            .as_deref()
            .and_then(|port| self.devices.iter().position(|d| d.port == port));
        if self.selected_device.is_none() && selected_port.is_some() {
            self.observation_device_association = cambium::TextInput::default();
        }
        self.survey = SurveyState::Surveyed;
        self.refusal.clear();
    }

    /// Select a device by index, clearing any refusal it might resolve.
    pub fn select_device(&mut self, index: usize) {
        if index < self.devices.len() {
            if self.selected_device != Some(index) {
                self.observation_device_association = cambium::TextInput::default();
            }
            self.selected_device = Some(index);
            self.selected_board_family = None;
            self.v4_product_profile = None;
            self.refusal.clear();
        }
    }

    /// Record the owner's board-family declaration for a silent port. This is
    /// deliberately separate from `select_device`: selecting a COM location
    /// never silently selects a board family.
    pub fn select_board_family(&mut self, family: BoardFamily) {
        if family != BoardFamily::HeltecV4 {
            self.v4_product_profile = None;
        }
        self.selected_board_family = Some(family);
        self.refusal.clear();
    }

    /// Adopt a board revision the owner selected from a visible, board-specific
    /// choice. This is still an owner claim: it neither inspects a port nor
    /// asks Linkboy to infer a revision from device evidence.
    pub fn select_board_revision(&mut self, revision: &str) {
        self.board_revision = cambium::TextInput::new(revision);
        self.v4_product_profile = None;
        self.refusal.clear();
    }

    /// Select the exact V4.2 schematic profile documented for the owner's Meshnology N39 kit.
    /// This does not generalize to another V4 carrier, and Linkboy still has to prove the
    /// ESP32-S3/16 MiB ROM-loader facts before it will make a plan.
    pub fn select_meshnology_n39_v4_2_profile(&mut self) {
        self.selected_board_family = Some(BoardFamily::HeltecV4);
        self.board_revision = cambium::TextInput::new("4.2");
        self.v4_product_profile = Some(V4ProductProfile::MeshnologyN39V42);
        self.refusal.clear();
    }

    /// Make the owner-confirmed selection that Signalman asks Linkboy to validate against its
    /// package and loader facts.
    pub fn board_selection(
        &self,
        family: BoardFamily,
        revision: impl Into<String>,
    ) -> linkboy::BoardSelection {
        let revision = revision.into();
        match (family.clone(), revision.as_str(), self.v4_product_profile) {
            (BoardFamily::HeltecV4, "4.2", Some(V4ProductProfile::MeshnologyN39V42)) => {
                linkboy::BoardSelection::documented_product_profile(
                    family,
                    revision,
                    MESHNOLOGY_N39_NAME,
                    MESHNOLOGY_N39_DOCUMENTATION_URL,
                )
            }
            _ => linkboy::BoardSelection::owner_confirmed(family, revision),
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
            FlashEvent::ManualCheckRequired { .. } => {
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
                self.recovery = Some(FirmwareInstallRecovery {
                    stage: stage_from_linkboy(&facts.stage),
                    last_known_port: facts.last_known_port.clone(),
                    write_started: facts.write_started,
                    after_failure: instructions.after_failure.clone(),
                });
                self.recovery_instructions = Some(instructions.after_failure.clone());
            }
            FlashEvent::Refused { reasons } => {
                self.install_running = false;
                self.refusal = reasons.iter().map(ToString::to_string).collect();
            }
            _ => {}
        }
    }

    /// Apply an update from Signalman's owned installer worker. Its raw Linkboy
    /// event remains inside Signalman; this face receives just the owner-facing
    /// message, progress, terminal result, and recovery stage it projects.
    pub fn apply_install_update(&mut self, update: FirmwareInstallUpdate) {
        match self.installer.apply_install_update(update) {
            FirmwareInstallNotice::Activity {
                line,
                progress,
                replaces_previous_progress,
            } => {
                if replaces_previous_progress
                    && self
                        .notes
                        .last()
                        .is_some_and(|last| last.starts_with("Writing "))
                {
                    self.notes.pop();
                }
                if let Some(line) = line {
                    self.notes.push(line);
                }
                if let Some(progress) = progress {
                    self.progress = Some(progress);
                }
            }
            FirmwareInstallNotice::Complete | FirmwareInstallNotice::ManualCheckRequired => {
                self.progress = Some(1.0);
                self.install_running = false;
                self.recovery = None;
                self.recovery_instructions = None;
            }
            FirmwareInstallNotice::RecoveryRequired { recovery } => {
                self.install_running = false;
                self.recovery_instructions = Some(recovery.after_failure.clone());
                self.recovery = Some(recovery);
            }
            FirmwareInstallNotice::Refused { reasons } => {
                self.install_running = false;
                self.refusal = reasons;
            }
            FirmwareInstallNotice::Failed(why) => self.worker_lost(&why),
            FirmwareInstallNotice::Finished if self.install_running => {
                self.worker_lost("the installer ended without a final receipt");
            }
            FirmwareInstallNotice::Finished => {}
        }
    }

    /// The worker died without a terminal event: a recovery situation, not a
    /// success.
    pub fn worker_lost(&mut self, why: &str) {
        self.install_running = false;
        self.notes.push(format!("The installer stopped: {why}"));
    }

    /// Decide whether the root window may close. A worker that is writing or
    /// verifying must stay attached to its process and device observation until
    /// it reaches a receipt or structured recovery state.
    pub fn close_disposition(&mut self) -> cambium_genet_winit_host::CloseDisposition {
        if self.observation_collecting {
            self.observation_notice =
                Some("Observation collection is active. Stop it before closing Signalman.".into());
            cambium_genet_winit_host::CloseDisposition::KeepVisible
        } else if self.install_running {
            self.refuse_with(vec![
                "Installation is still active. Keep Signalman open until it completes or shows recovery instructions.".into(),
            ]);
            cambium_genet_winit_host::CloseDisposition::KeepVisible
        } else {
            cambium_genet_winit_host::CloseDisposition::Exit
        }
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
            FirmwareInstallStage::Preparing => "while preparing",
            FirmwareInstallStage::EnteringBootloader => "while entering the bootloader",
            FirmwareInstallStage::Transfer => "during the transfer",
            FirmwareInstallStage::Rebooting => "while rebooting",
            FirmwareInstallStage::VerifyingApplication => "while verifying the application",
        })
    }
}

fn stage_from_linkboy(stage: &linkboy::ExecutionStage) -> FirmwareInstallStage {
    match stage {
        linkboy::ExecutionStage::Preparing => FirmwareInstallStage::Preparing,
        linkboy::ExecutionStage::EnteringBootloader => FirmwareInstallStage::EnteringBootloader,
        linkboy::ExecutionStage::Transfer => FirmwareInstallStage::Transfer,
        linkboy::ExecutionStage::Rebooting => FirmwareInstallStage::Rebooting,
        linkboy::ExecutionStage::VerifyingApplication => FirmwareInstallStage::VerifyingApplication,
    }
}
