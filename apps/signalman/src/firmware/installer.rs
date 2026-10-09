//! The owner flow, its install worker, and the package catalog.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::JoinHandle;

use linkboy::{
    CatalogError, CatalogPackage, DeviceObservation, FlashEvent, FlashPackage, FlashPlan,
    FlashRange, FlowError, OwnerFlow, OwnerStage, PackageIndex, ReceiptResult, StateImpact,
};

use super::{describe_event, event_progress};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareReview {
    pub package_id: String,
    pub display_name: String,
    pub version: String,
    pub publisher: String,
    pub package_parts: Vec<linkboy::PackagePartIdentity>,
    pub publisher_signature: Option<linkboy::PublisherSignature>,
    pub license: String,
    pub source_url: String,
    pub origin_url: String,
    pub board: String,
    pub board_revision: String,
    /// Why Linkboy accepts the otherwise-unobservable carrier revision.
    pub board_revision_evidence: String,
    pub route: String,
    pub helper: String,
    pub helper_version: String,
    pub helper_license: String,
    pub helper_source_url: String,
    pub write_ranges: Vec<FlashRange>,
    pub preserved_ranges: Vec<FlashRange>,
    pub state_impact: StateImpact,
    pub recovery_before_write: String,
    pub recovery_after_failure: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareView {
    pub stage: OwnerStage,
    pub title: &'static str,
    pub device: Option<String>,
    pub package: Option<String>,
    pub route: Option<String>,
    pub state_impact: Option<String>,
    pub review: Option<FirmwareReview>,
    pub recovery_detail: Option<String>,
    pub result: Option<ReceiptResult>,
}

pub struct FirmwareInstaller {
    flow: OwnerFlow,
}

pub struct FirmwareCatalog {
    index_path: PathBuf,
    index: PackageIndex,
}

/// A host-neutral request to drain a product-owned worker (the Armillary actor callback shape).
pub type InstallerWake = Arc<dyn Fn() + Send + Sync>;

// FlashEvent's recovery variant is deliberately wide; see linkboy's ExecutionError.
#[allow(clippy::large_enum_variant)]
enum WorkerMessage {
    Event(FlashEvent),
    Failed(String),
    Finished,
}

/// An update delivered by Signalman's approved-install worker.
///
/// Its Linkboy executor message is private; a face hands it back to
/// [`FirmwareInstaller::apply_install_update`] for an owner-facing [`FirmwareInstallNotice`].
pub struct FirmwareInstallUpdate(WorkerMessage);

/// The public execution stage a recovery reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareInstallStage {
    Preparing,
    EnteringBootloader,
    Transfer,
    Rebooting,
    VerifyingApplication,
}

/// Recovery facts a face needs without exposing Linkboy's executor record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareInstallRecovery {
    pub stage: FirmwareInstallStage,
    pub last_known_port: Option<String>,
    pub write_started: bool,
    pub after_failure: String,
}

/// A face-facing result of applying an installer worker update.
#[derive(Clone, Debug, PartialEq)]
pub enum FirmwareInstallNotice {
    /// A nonterminal status line, with write progress where the executor has one.
    Activity {
        line: Option<String>,
        progress: Option<f32>,
        replaces_previous_progress: bool,
    },
    /// The flow now owns a verified completion receipt.
    Complete,
    /// The package transferred, but its own interface requires a manual check.
    ManualCheckRequired,
    /// The run reached a recovery boundary with owner-readable next steps.
    RecoveryRequired { recovery: FirmwareInstallRecovery },
    /// Linkboy refused the approved run before a write could continue.
    Refused { reasons: Vec<String> },
    /// The worker stopped with an error that had no structured terminal event.
    Failed(String),
    /// The worker thread ended. A terminal receipt should have arrived first.
    Finished,
}

/// The UI-thread handle to one blocking Linkboy installation.
///
/// The worker owns only copies of the approved inputs; the caller drains it after its host
/// wakes, so no renderer or application state crosses the thread boundary.
pub struct FirmwareInstallWorker {
    channel: Option<Receiver<WorkerMessage>>,
    handle: Option<JoinHandle<()>>,
}

impl FirmwareInstallWorker {
    fn start(plan: FlashPlan, package: FlashPackage, wake: InstallerWake) -> Result<Self, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("signalman-installer".into())
            .spawn(move || {
                let mut process = linkboy::SystemProcessRunner::default();
                let mut device = linkboy::LiveDeviceRunner;
                let emit_tx = tx.clone();
                let emit_wake = wake.clone();
                let mut emit = move |event: FlashEvent| {
                    if emit_tx.send(WorkerMessage::Event(event)).is_ok() {
                        (emit_wake)();
                    }
                };
                let result = linkboy::execute_plan(
                    &plan,
                    &package,
                    &mut process,
                    &mut device,
                    linkboy::executor::DEFAULT_PATIENCE,
                    &mut emit,
                );
                if let Err(error) = result {
                    // Recovery emits its own terminal event; reporting it again would duplicate it.
                    if !matches!(error, linkboy::ExecutionError::RecoveryRequired { .. })
                        && tx.send(WorkerMessage::Failed(error.to_string())).is_ok()
                    {
                        (wake)();
                    }
                }
                if tx.send(WorkerMessage::Finished).is_ok() {
                    (wake)();
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            channel: Some(rx),
            handle: Some(handle),
        })
    }

    /// Take every update currently available. Never blocks the host thread.
    pub fn drain(&mut self) -> Vec<FirmwareInstallUpdate> {
        let mut updates = Vec::new();
        let Some(channel) = self.channel.as_ref() else {
            return updates;
        };
        loop {
            match channel.try_recv() {
                Ok(message @ WorkerMessage::Event(_)) => {
                    updates.push(FirmwareInstallUpdate(message));
                }
                Ok(message @ WorkerMessage::Failed(_)) => {
                    updates.push(FirmwareInstallUpdate(message));
                }
                Ok(WorkerMessage::Finished) => {
                    updates.push(FirmwareInstallUpdate(WorkerMessage::Finished));
                    self.channel = None;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    updates.push(FirmwareInstallUpdate(WorkerMessage::Finished));
                    self.channel = None;
                    break;
                }
            }
        }
        if self.channel.is_none()
            && self
                .handle
                .as_ref()
                .is_some_and(|handle| handle.is_finished())
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
        updates
    }

    /// Whether the install has not yet emitted its terminal worker message.
    pub fn running(&self) -> bool {
        self.channel.is_some()
    }
}

impl FirmwareCatalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, CatalogError> {
        let index_path = path.as_ref().to_path_buf();
        let index = PackageIndex::load(&index_path)?;
        index.verify_packages(&index_path)?;
        Ok(Self { index_path, index })
    }

    pub fn packages(&self) -> &[CatalogPackage] {
        &self.index.packages
    }

    pub fn package(&self, package_id: &str) -> Option<&CatalogPackage> {
        self.index.package(package_id)
    }

    pub fn load_package(&self, package_id: &str) -> Result<FlashPackage, CatalogError> {
        self.index.load_package(&self.index_path, package_id)
    }
}

// Inherits Linkboy's deliberately large recovery payloads; see linkboy's ExecutionError.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum FirmwareError {
    Catalog(CatalogError),
    Flow(FlowError),
    /// A loader run that could not produce the facts a plan needs.
    Execution(linkboy::ExecutionError),
    Discovery(linkboy::DiscoveryError),
    LoaderSnapshot(String),
    Worker(String),
}

impl std::fmt::Display for FirmwareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Catalog(error) => error.fmt(formatter),
            Self::Flow(error) => error.fmt(formatter),
            Self::Execution(error) => error.fmt(formatter),
            Self::Discovery(error) => error.fmt(formatter),
            Self::LoaderSnapshot(error) => formatter.write_str(error),
            Self::Worker(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for FirmwareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Catalog(error) => Some(error),
            Self::Flow(error) => Some(error),
            Self::Execution(error) => Some(error),
            Self::Discovery(error) => Some(error),
            Self::LoaderSnapshot(_) => None,
            Self::Worker(_) => None,
        }
    }
}

impl From<CatalogError> for FirmwareError {
    fn from(error: CatalogError) -> Self {
        Self::Catalog(error)
    }
}

impl From<FlowError> for FirmwareError {
    fn from(error: FlowError) -> Self {
        Self::Flow(error)
    }
}

impl Default for FirmwareInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl FirmwareInstaller {
    pub fn new() -> Self {
        Self {
            flow: OwnerFlow::new(),
        }
    }

    pub fn view(&self) -> FirmwareView {
        let stage = self.flow.stage();
        let plan = self.flow.approved_plan();
        let device = self
            .flow
            .observation()
            .map(|observation| match &observation.transport {
                linkboy::DeviceTransport::SerialPort(port) => format!("serial:{port}"),
                linkboy::DeviceTransport::SerialDfuPort(port) => {
                    format!("serial-dfu:{port}")
                }
                linkboy::DeviceTransport::MountedVolume(volume) => format!("volume:{volume}"),
            });
        let package = self.flow.package().map(|package| {
            format!(
                "{} {}",
                package.manifest().display_name,
                package.manifest().version
            )
        });
        let review = match (plan, self.flow.package()) {
            (Some(plan), Some(package)) => Some(review(plan, package)),
            _ => None,
        };
        FirmwareView {
            stage,
            title: title(stage),
            device,
            package,
            route: plan.map(|plan| plan.route().to_string()),
            state_impact: plan.map(|plan| plan.state_impact().to_string()),
            review,
            recovery_detail: self.flow.recovery().map(|facts| facts.detail.clone()),
            result: self.flow.receipt().map(|receipt| receipt.result.clone()),
        }
    }

    pub fn choose_device(&mut self, observation: DeviceObservation) -> Result<(), FlowError> {
        self.flow.choose_device(observation)
    }

    pub fn choose_firmware(&mut self, package: FlashPackage) -> Result<(), FlowError> {
        self.flow.choose_firmware(package)
    }

    pub fn choose_catalog_firmware(
        &mut self,
        catalog: &FirmwareCatalog,
        package_id: &str,
    ) -> Result<(), FirmwareError> {
        self.choose_firmware(catalog.load_package(package_id)?)?;
        Ok(())
    }

    pub fn approve_changes(&mut self) -> Result<(), FlowError> {
        self.flow.approve_changes()
    }

    pub fn begin_install(&mut self) -> Result<(&FlashPlan, &FlashPackage), FlowError> {
        self.flow.begin_install()
    }

    /// Start exactly the plan the owner flow approved. The face supplies only a wake callback.
    pub fn start_install(
        &mut self,
        wake: InstallerWake,
    ) -> Result<FirmwareInstallWorker, FirmwareError> {
        let (plan, package) = self.flow.begin_install()?;
        FirmwareInstallWorker::start(plan.clone(), package.clone(), wake)
            .map_err(FirmwareError::Worker)
    }

    /// Apply one worker update to the owning flow and return only the presentation facts a face
    /// needs. A GUI never matches Linkboy's executor protocol or manufactures a receipt.
    pub fn apply_install_update(&mut self, update: FirmwareInstallUpdate) -> FirmwareInstallNotice {
        match update.0 {
            WorkerMessage::Event(event) => {
                let notice = match &event {
                    FlashEvent::Complete { .. } => FirmwareInstallNotice::Complete,
                    FlashEvent::ManualCheckRequired { .. } => {
                        FirmwareInstallNotice::ManualCheckRequired
                    }
                    FlashEvent::RecoveryRequired {
                        facts,
                        instructions,
                        ..
                    } => FirmwareInstallNotice::RecoveryRequired {
                        recovery: FirmwareInstallRecovery {
                            stage: install_stage(&facts.stage),
                            last_known_port: facts.last_known_port.clone(),
                            write_started: facts.write_started,
                            after_failure: instructions.after_failure.clone(),
                        },
                    },
                    FlashEvent::Refused { reasons } => FirmwareInstallNotice::Refused {
                        reasons: reasons.iter().map(ToString::to_string).collect(),
                    },
                    _ => FirmwareInstallNotice::Activity {
                        line: describe_event(&event),
                        progress: event_progress(&event),
                        replaces_previous_progress: matches!(&event, FlashEvent::Writing { .. }),
                    },
                };
                self.flow.apply_event(&event);
                notice
            }
            WorkerMessage::Failed(message) => FirmwareInstallNotice::Failed(message),
            WorkerMessage::Finished => FirmwareInstallNotice::Finished,
        }
    }

    pub fn apply_event(&mut self, event: &FlashEvent) {
        self.flow.apply_event(event);
    }

    /// The finished receipt, once the flow has one. [`FirmwareView`] carries only its result.
    pub fn receipt(&self) -> Option<&linkboy::FlashReceipt> {
        self.flow.receipt()
    }

    /// The approved plan, once one exists. Shared, never owned.
    pub fn plan(&self) -> Option<&FlashPlan> {
        self.flow.approved_plan()
    }

    /// The chosen package, once one exists.
    pub fn chosen_package(&self) -> Option<&FlashPackage> {
        self.flow.package()
    }
}

fn install_stage(stage: &linkboy::ExecutionStage) -> FirmwareInstallStage {
    match stage {
        linkboy::ExecutionStage::Preparing => FirmwareInstallStage::Preparing,
        linkboy::ExecutionStage::EnteringBootloader => FirmwareInstallStage::EnteringBootloader,
        linkboy::ExecutionStage::Transfer => FirmwareInstallStage::Transfer,
        linkboy::ExecutionStage::Rebooting => FirmwareInstallStage::Rebooting,
        linkboy::ExecutionStage::VerifyingApplication => FirmwareInstallStage::VerifyingApplication,
    }
}

fn review(plan: &FlashPlan, package: &FlashPackage) -> FirmwareReview {
    let manifest = package.manifest();
    let helper = manifest.helper_for(plan.route());
    FirmwareReview {
        package_id: manifest.package_id.clone(),
        display_name: manifest.display_name.clone(),
        version: manifest.version.clone(),
        publisher: manifest.publisher.clone(),
        package_parts: plan.parts().to_vec(),
        publisher_signature: plan.package().publisher_signature.clone(),
        license: manifest.license.clone(),
        source_url: manifest.source_url.clone(),
        origin_url: manifest.origin_url.clone(),
        board: plan.board().family.to_string(),
        board_revision: plan.board().revision.clone(),
        board_revision_evidence: plan.board().evidence.describe(),
        route: plan.route().to_string(),
        helper: plan.helper().to_string(),
        helper_version: helper
            .map(|helper| helper.version.clone())
            .unwrap_or_default(),
        helper_license: helper
            .map(|helper| helper.license.clone())
            .unwrap_or_default(),
        helper_source_url: helper
            .map(|helper| helper.source_url.clone())
            .unwrap_or_default(),
        write_ranges: plan.write_ranges().to_vec(),
        preserved_ranges: plan.preserved_ranges().to_vec(),
        state_impact: plan.state_impact().clone(),
        recovery_before_write: plan.recovery_before_write().to_string(),
        recovery_after_failure: plan.recovery_after_failure().to_string(),
    }
}

fn title(stage: OwnerStage) -> &'static str {
    match stage {
        OwnerStage::ChooseDevice => "Choose device",
        OwnerStage::ChooseFirmware => "Choose firmware",
        OwnerStage::ReviewChanges => "Review changes",
        OwnerStage::PrepareDevice => "Prepare the device",
        OwnerStage::Install => "Install",
        OwnerStage::VerifyOrRecover => "Verify or recover",
    }
}
