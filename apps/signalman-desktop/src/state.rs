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

use std::collections::BTreeSet;
use std::time::Duration;

use linkboy::{BoardFamily, FlashEvent, FlashReceipt, OwnerStage, ReceiptResult};
use seiche::{LayoutSnapshot, NodeKey};
use signalman::management::{
    ManagementMaterial, ManagementNodeId, ManagementPresence, ManagementRelationId, StalePolicy,
};
use signalman::{
    DeviceCandidate, FirmwareCatalog, FirmwareInstallNotice, FirmwareInstallRecovery,
    FirmwareInstallStage, FirmwareInstallUpdate, FirmwareInstaller, FirmwareView, describe_event,
    event_progress, refusal_lines,
};

use crate::device_mere::{DeviceMere, DeviceProjection, ReconcileReceipt};
use crate::network::{
    NetworkInput, NetworkLayout, NetworkPhysics, accept_layout, input_from_projection,
    swatch_from_projection, world_from_normalized,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DesktopSection {
    #[default]
    Devices,
    Network,
    Messages,
    Map,
    Browse,
}

/// The pinned Cambium canvas has one honest label-density seam: labels shown
/// or hidden. More density levels require an upstream component change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelDensity {
    Hidden,
    #[default]
    Shown,
}

/// Owner policy for the management surface. These defaults are initial values
/// shown by the shell, not private constants that silently override a choice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManagementSettings {
    pub stale_age_minutes: u32,
    pub announce_history_bound: usize,
    pub force_strength: f32,
    pub layout_damping: f32,
    pub label_density: LabelDensity,
    pub show_last_known: bool,
}

impl Default for ManagementSettings {
    fn default() -> Self {
        Self {
            stale_age_minutes: 15,
            announce_history_bound: 256,
            force_strength: 1.0,
            layout_damping: 2.5,
            label_density: LabelDensity::Shown,
            show_last_known: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NetworkRequest {
    Reconcile(NetworkInput),
    Pin(NodeKey, euclid::default::Point2D<f32>),
    Unpin(NodeKey),
}

/// An externally documented carrier profile, selected by the owner instead of inferred from a
/// serial transport. Each variant is intentionally package-specific.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V4ProductProfile {
    MeshnologyN39V42,
}

pub const MESHNOLOGY_N39_NAME: &str = "Meshnology N39 WiFi LoRa 32 V4 kit";
pub const MESHNOLOGY_N39_DOCUMENTATION_URL: &str =
    "https://wiki.meshnology.com/N39/Meshnology%20N39/";

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
    /// Take an explicitly named T114 UF2 volume into the flow.
    ConfirmMountedT114,
    /// Take an owner-confirmed port already running the captured T114 DFU loader into the flow.
    ConfirmT114Dfu,
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
    pub section: DesktopSection,
    pub management_settings: ManagementSettings,
    pub device_mere: DeviceMere,
    pub network_epoch: u64,
    pub network_layout: Option<LayoutSnapshot>,
    pub network_pan: (f32, f32),
    pub network_zoom: f32,
    pub selected_relation: Option<ManagementRelationId>,
    pub pending_network: Option<NetworkRequest>,

    /// The owner flow. The only thing that can move a page.
    pub installer: FirmwareInstaller,
    /// The verified package catalog, or why it could not be loaded.
    pub catalog: Option<FirmwareCatalog>,
    pub catalog_error: Option<String>,

    pub devices: Vec<DeviceCandidate>,
    pub survey: SurveyState,
    pub selected_device: Option<usize>,
    pub selected_package: Option<usize>,
    /// An owner declaration for a silent serial device. A discovered Retinue
    /// banner remains Linkboy evidence; this is only the escape hatch for a
    /// foreign application that cannot name itself.
    pub selected_board_family: Option<BoardFamily>,
    /// The board revision the owner types. A plan is refused without it, and
    /// that refusal is shown rather than hidden behind a disabled control.
    pub board_revision: cambium::TextInput,
    /// A narrowly named, externally documented source for a revision. This is distinct from a
    /// typed carrier marking so the approved plan says why either claim is allowed.
    pub v4_product_profile: Option<V4ProductProfile>,
    /// A mounted `HT-n5262` UF2 volume, entered explicitly because a drive
    /// letter is a transport location rather than an inferred board identity.
    pub t114_uf2_volume: cambium::TextInput,
    /// Where the GUI retains the mounted bootloader record for a later serial
    /// DFU recovery. This is required for a silent foreign T114 plan.
    pub t114_loader_record: cambium::TextInput,

    /// The current refusal, as separate visible lines. Cleared when the owner
    /// changes something that could resolve it.
    pub refusal: Vec<String>,
    /// The execution event log, oldest first.
    pub notes: Vec<String>,
    /// Transfer progress, `0.0..=1.0`, while one is running.
    pub progress: Option<f32>,
    /// The last Signalman-owned execution stage a recovery reported.
    pub recovery: Option<FirmwareInstallRecovery>,
    pub recovery_instructions: Option<String>,
    /// Set once the plan has been handed to the worker, so it is handed over
    /// once and the Install page can say it is running.
    pub install_running: bool,

    /// What the view asked the application loop to do.
    pub pending: Option<Request>,
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
            section: DesktopSection::Devices,
            management_settings: ManagementSettings::default(),
            device_mere: DeviceMere::new(),
            network_epoch: 0,
            network_layout: None,
            network_pan: (0.0, 0.0),
            network_zoom: 1.0,
            selected_relation: None,
            pending_network: None,
            installer: FirmwareInstaller::new(),
            catalog,
            catalog_error,
            devices: Vec::new(),
            survey: SurveyState::default(),
            selected_device: None,
            selected_package: None,
            selected_board_family: None,
            board_revision: cambium::TextInput::default(),
            v4_product_profile: None,
            t114_uf2_volume: cambium::TextInput::default(),
            t114_loader_record: cambium::TextInput::default(),
            refusal: Vec::new(),
            notes: Vec::new(),
            progress: None,
            recovery: None,
            recovery_instructions: None,
            install_running: false,
            pending: None,
        }
    }

    pub fn show_section(&mut self, section: DesktopSection) {
        self.section = section;
    }

    /// Apply one pure management projection to the app-owned Mere. Only a
    /// topology change restarts physics; refreshed payload and stale markings
    /// remain graph edits without disturbing the current layout.
    pub fn apply_management_material(&mut self, material: &ManagementMaterial) -> ReconcileReceipt {
        let previous_projection = self.network_projection();
        let receipt = self.device_mere.reconcile(material);
        let projection = self.network_projection();
        if self.device_mere.selected().is_some_and(|selected| {
            !projection
                .nodes
                .iter()
                .any(|node| &node.fact.id == selected)
        }) {
            self.device_mere.select(None);
        }
        if self.selected_relation.as_ref().is_some_and(|selected| {
            !projection
                .relations
                .iter()
                .any(|relation| &relation.id == selected)
        }) {
            self.selected_relation = None;
        }
        if !same_visible_topology(&previous_projection, &projection) {
            self.queue_network_reconcile(&projection);
        }
        receipt
    }

    pub fn network_projection(&self) -> DeviceProjection {
        let mut projection = self.device_mere.projection();
        if self.management_settings.show_last_known {
            return projection;
        }
        projection
            .nodes
            .retain(|node| node.fact.presence == ManagementPresence::Live);
        let visible = projection
            .nodes
            .iter()
            .map(|node| node.key)
            .collect::<BTreeSet<_>>();
        projection
            .relations
            .retain(|relation| visible.contains(&relation.from) && visible.contains(&relation.to));
        projection
    }

    pub fn network_swatch(
        &self,
    ) -> cambium::GraphCanvasSwatch<ManagementNodeId, signalman::management::ManagementPresence>
    {
        let projection = self.network_projection();
        swatch_from_projection(
            &projection,
            self.network_layout.as_ref(),
            self.device_mere.selected(),
            self.network_pan,
            self.network_zoom,
            self.management_settings.label_density == LabelDensity::Shown,
        )
    }

    pub fn take_network_request(&mut self) -> Option<NetworkRequest> {
        self.pending_network.take()
    }

    pub fn adopt_network_layout(&mut self, layout: NetworkLayout) -> bool {
        let Some(snapshot) = accept_layout(self.network_epoch, layout) else {
            return false;
        };
        self.network_layout = Some(snapshot);
        true
    }

    pub fn select_network_node(&mut self, id: ManagementNodeId) {
        self.device_mere.select(Some(id));
        self.selected_relation = None;
    }

    pub fn select_network_relation(&mut self, id: &str) {
        let projection = self.network_projection();
        self.selected_relation = projection
            .relations
            .iter()
            .find(|relation| relation.id.as_str() == id)
            .map(|relation| relation.id.clone());
    }

    pub fn drag_network_node(
        &mut self,
        id: &ManagementNodeId,
        phase: cambium::PointerPhase,
        normalized: (f32, f32),
    ) {
        let Some(key) = self
            .network_projection()
            .nodes
            .iter()
            .find(|node| &node.fact.id == id)
            .map(|node| node.key)
        else {
            return;
        };
        self.pending_network = Some(match phase {
            cambium::PointerPhase::Down | cambium::PointerPhase::Move => {
                NetworkRequest::Pin(key, world_from_normalized(normalized))
            }
            cambium::PointerPhase::Up => NetworkRequest::Unpin(key),
        });
    }

    pub fn pan_network(&mut self, dx: f32, dy: f32) {
        self.network_pan.0 = (self.network_pan.0 + dx).clamp(-1.0, 1.0);
        self.network_pan.1 = (self.network_pan.1 + dy).clamp(-1.0, 1.0);
    }

    pub fn zoom_network(&mut self, factor: f32) {
        self.network_zoom = (self.network_zoom * factor).clamp(0.5, 3.0);
    }

    pub fn reset_network_view(&mut self) {
        self.network_pan = (0.0, 0.0);
        self.network_zoom = 1.0;
    }

    /// The stale policy a future station-snapshot lease must pass to
    /// `project_management`. The current exact Mere pin exposes no such lease,
    /// so this remains an explicit source-side seam rather than fake live data.
    pub fn stale_policy(&self) -> StalePolicy {
        StalePolicy {
            after: Duration::from_secs(u64::from(self.management_settings.stale_age_minutes) * 60),
        }
    }

    /// Postilion reads this bound when the station opens. It has no runtime
    /// setter, so the shell labels this value as applying to the next connection.
    pub fn announce_history_bound(&self) -> usize {
        self.management_settings.announce_history_bound
    }

    pub fn shorten_stale_age(&mut self) {
        self.management_settings.stale_age_minutes = self
            .management_settings
            .stale_age_minutes
            .saturating_sub(5)
            .max(1);
    }

    pub fn lengthen_stale_age(&mut self) {
        self.management_settings.stale_age_minutes = self
            .management_settings
            .stale_age_minutes
            .saturating_add(5)
            .min(10_080);
    }

    pub fn reduce_history_bound(&mut self) {
        self.management_settings.announce_history_bound =
            (self.management_settings.announce_history_bound / 2).max(16);
    }

    pub fn increase_history_bound(&mut self) {
        self.management_settings.announce_history_bound = self
            .management_settings
            .announce_history_bound
            .saturating_mul(2)
            .min(4096);
    }

    pub fn reduce_force_strength(&mut self) {
        self.management_settings.force_strength =
            (self.management_settings.force_strength * 0.8).max(0.25);
        self.reconfigure_network_physics();
    }

    pub fn increase_force_strength(&mut self) {
        self.management_settings.force_strength =
            (self.management_settings.force_strength * 1.25).min(4.0);
        self.reconfigure_network_physics();
    }

    pub fn reduce_layout_damping(&mut self) {
        self.management_settings.layout_damping =
            (self.management_settings.layout_damping - 0.5).max(0.5);
        self.reconfigure_network_physics();
    }

    pub fn increase_layout_damping(&mut self) {
        self.management_settings.layout_damping =
            (self.management_settings.layout_damping + 0.5).min(8.0);
        self.reconfigure_network_physics();
    }

    pub fn toggle_network_labels(&mut self) {
        self.management_settings.label_density = match self.management_settings.label_density {
            LabelDensity::Hidden => LabelDensity::Shown,
            LabelDensity::Shown => LabelDensity::Hidden,
        };
    }

    pub fn toggle_last_known(&mut self) {
        self.management_settings.show_last_known = !self.management_settings.show_last_known;
        if !self.management_settings.show_last_known {
            let projection = self.network_projection();
            if self.device_mere.selected().is_some_and(|selected| {
                !projection
                    .nodes
                    .iter()
                    .any(|node| &node.fact.id == selected)
            }) {
                self.device_mere.select(None);
            }
            if self.selected_relation.as_ref().is_some_and(|selected| {
                !projection
                    .relations
                    .iter()
                    .any(|relation| &relation.id == selected)
            }) {
                self.selected_relation = None;
            }
        }
        let projection = self.network_projection();
        self.queue_network_reconcile(&projection);
    }

    pub fn reset_management_settings(&mut self) {
        let previous = self.management_settings;
        self.management_settings = ManagementSettings::default();
        if previous.force_strength != self.management_settings.force_strength
            || previous.layout_damping != self.management_settings.layout_damping
            || previous.show_last_known != self.management_settings.show_last_known
        {
            let projection = self.network_projection();
            self.queue_network_reconcile(&projection);
        }
    }

    fn reconfigure_network_physics(&mut self) {
        let projection = self.network_projection();
        self.queue_network_reconcile(&projection);
    }

    fn queue_network_reconcile(&mut self, projection: &DeviceProjection) {
        self.network_epoch = self.network_epoch.saturating_add(1);
        let physics = NetworkPhysics {
            force_strength: self.management_settings.force_strength,
            linear_damping: self.management_settings.layout_damping,
        };
        self.pending_network = Some(NetworkRequest::Reconcile(input_from_projection(
            projection,
            self.network_layout.as_ref(),
            self.network_epoch,
            physics,
        )));
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
        self.selected_device =
            selected_port.and_then(|port| self.devices.iter().position(|d| d.port == port));
        self.survey = SurveyState::Surveyed;
        self.refusal.clear();
    }

    /// Select a device by index, clearing any refusal it might resolve.
    pub fn select_device(&mut self, index: usize) {
        if index < self.devices.len() {
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

    /// The worker died without a terminal event. Said plainly, because a
    /// transfer that stopped for an unknown reason is a recovery situation and
    /// not a success.
    pub fn worker_lost(&mut self, why: &str) {
        self.install_running = false;
        self.notes.push(format!("The installer stopped: {why}"));
    }

    /// Decide whether the root window may close. A worker that is writing or
    /// verifying must stay attached to its process and device observation until
    /// it reaches a receipt or structured recovery state.
    pub fn close_disposition(&mut self) -> cambium_genet_winit_host::CloseDisposition {
        if self.install_running {
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

fn same_visible_topology(left: &DeviceProjection, right: &DeviceProjection) -> bool {
    left.nodes
        .iter()
        .map(|node| node.key)
        .eq(right.nodes.iter().map(|node| node.key))
        && left
            .relations
            .iter()
            .map(|relation| {
                (
                    &relation.id,
                    relation.from,
                    relation.to,
                    &relation.fact.kind,
                )
            })
            .eq(right.relations.iter().map(|relation| {
                (
                    &relation.id,
                    relation.from,
                    relation.to,
                    &relation.fact.kind,
                )
            }))
}
