//! Signalman's projection of Linkboy's owner installer.
//!
//! This is deliberately semantic rather than terminal-shaped. A future window can render the
//! view with its own controls while Linkboy remains the only owner of package policy, plans, and
//! execution events.

// Firmware errors inherit Linkboy's deliberately large recovery payloads; flashing is a
// cold path where truncated evidence costs more than a wide Err.
#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::JoinHandle;

use linkboy::{
    CatalogError, CatalogPackage, DeviceObservation, FlashEvent, FlashPackage, FlashPlan,
    FlashRange, FlowError, OwnerFlow, OwnerStage, PackageIndex, ReceiptResult, StateImpact,
};

/// One device an owner can choose, as Signalman would say it.
///
/// A port is a location, not an identity: the board is what the banner said,
/// and `known` records whether this build knows how to flash it. A port that
/// answers nothing is still listed, because a board that has stopped talking is
/// exactly the one an owner needs to recover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCandidate {
    /// The transport address (`COM7`, `/dev/ttyUSB0`).
    pub port: String,
    /// What the board said it is, or `None` for a silent port.
    pub board: Option<String>,
    /// The banner verbatim, for a person to read when the board is unknown.
    pub banner: String,
    pub region: Option<String>,
    pub channel: Option<String>,
    /// Whether this build can flash it.
    pub known: bool,
}

impl DeviceCandidate {
    /// The one-line description a chooser shows.
    pub fn summary(&self) -> String {
        match (&self.board, self.known) {
            (Some(board), true) => {
                let mut line = format!("{} — {board}", self.port);
                if let Some(region) = &self.region {
                    line.push_str(&format!(", region {region}"));
                }
                if let Some(channel) = &self.channel {
                    line.push_str(&format!(", channel {channel}"));
                }
                line
            }
            (Some(board), false) => {
                format!("{} — {board} (this build cannot flash it)", self.port)
            }
            (None, _) => format!("{} — silent (not running, or in use)", self.port),
        }
    }
}

/// Every serial port this machine has, asked what it is.
///
/// This is Signalman's device vocabulary over Linkboy's survey. It exists so a
/// face never reaches past Signalman into Linkboy for the one thing a chooser
/// needs; every decision about what a port *is* stays Linkboy's.
///
/// A machine with no ports is an empty list, not an error: "nothing plugged in"
/// is a state to show, not a failure to report.
pub fn survey_devices() -> Vec<DeviceCandidate> {
    let Ok(ports) = linkboy::ports() else {
        return Vec::new();
    };
    survey_ports(ports)
}

/// Ask only the named serial ports what they are.
///
/// The graphical application normally calls [`survey_devices`]. An owner who
/// has already selected the physical port can instead use this narrow survey,
/// which leaves every other serial device alone. The observations remain
/// Linkboy's; this function only defines the survey set.
pub fn survey_ports(ports: impl IntoIterator<Item = String>) -> Vec<DeviceCandidate> {
    ports
        .into_iter()
        .map(|port| {
            let found = linkboy::identify(&port);
            let known = matches!(
                found.board,
                Some(linkboy::Board::HeltecV4 | linkboy::Board::T114)
            );
            DeviceCandidate {
                port: found.port,
                board: found.board.map(|board| match board {
                    linkboy::Board::Unknown(line) => line,
                    other => format!("{other:?}"),
                }),
                banner: found.banner,
                region: found.region,
                channel: found.channel,
                known,
            }
        })
        .collect()
}

/// The device observation for a chosen port, with whatever the loader can add.
///
/// Wraps the same construction `linkboy plan` and `linkboy flash` perform, so a
/// graphical face gets the same evidence a terminal run would — including the
/// ESP ROM discovery pass, without which a V4 plan is refused for missing
/// processor and flash facts.
pub fn observe_device(
    port: &str,
    selection: Option<(linkboy::BoardFamily, String)>,
) -> Result<DeviceObservation, FirmwareError> {
    observe_device_with_t114_loader_snapshot(port, selection, None)
}

/// Observe a serial application port with an optional T114 loader record captured from that
/// same board's mounted UF2 interface. The record is required for a silent foreign T114:
/// serial DFU does not report the processor, capacity, or SoftDevice facts on its own.
pub fn observe_device_with_t114_loader_snapshot(
    port: &str,
    selection: Option<(linkboy::BoardFamily, String)>,
    loader_snapshot: Option<&linkboy::T114LoaderSnapshot>,
) -> Result<DeviceObservation, FirmwareError> {
    observe_device_with_board_selection_and_t114_loader_snapshot(
        port,
        selection
            .map(|(family, revision)| linkboy::BoardSelection::owner_confirmed(family, revision)),
        loader_snapshot,
    )
}

/// Observe a serial application port with a complete owner-confirmed board selection.
///
/// This preserves the basis for a revision claim in the immutable plan. A documented product
/// profile is accepted only as that named profile, never as permission to treat every board in
/// the same family as interchangeable.
pub fn observe_device_with_board_selection_and_t114_loader_snapshot(
    port: &str,
    selection: Option<linkboy::BoardSelection>,
    loader_snapshot: Option<&linkboy::T114LoaderSnapshot>,
) -> Result<DeviceObservation, FirmwareError> {
    let found = linkboy::identify(port);
    let mut observation = DeviceObservation::from_found(&found);
    if let Some(selection) = selection.clone() {
        observation = observation.confirm_board_selection(selection);
    }
    if let Some(snapshot) = loader_snapshot {
        if !matches!(
            selection.as_ref().map(|selection| &selection.family),
            Some(linkboy::BoardFamily::T114)
        ) {
            return Err(FirmwareError::LoaderSnapshot(
                "a T114 loader record requires an explicit t114@revision selection".into(),
            ));
        }
        let facts = snapshot.serial_dfu_observation();
        observation = observation.with_hardware(linkboy::HardwareFacts {
            processor: facts.processor.clone(),
            flash_size: facts.flash_size,
            bootloader: facts.bootloader.clone(),
            loader_route: Some("captured-t114-loader-snapshot".into()),
            bootloader_usb: Some(facts),
        });
    }
    if linkboy::needs_esp_rom_probe(&observation) {
        let mut process = linkboy::SystemProcessRunner::default();
        let facts = linkboy::route::esp_rom::discover(&mut process, port)
            .map_err(|error| FirmwareError::Execution(linkboy::ExecutionError::Process(error)))?;
        observation = observation.with_hardware(linkboy::HardwareFacts {
            processor: facts.processor.clone(),
            flash_size: facts.flash_size,
            bootloader: facts.bootloader.clone(),
            loader_route: Some("esp-rom".into()),
            bootloader_usb: Some(facts),
        });
    }
    Ok(observation)
}

/// Observe an owner-selected port that is already running the T114 serial-DFU loader.
///
/// The retained loader record supplies the hardware facts, while the explicit transport state
/// tells Linkboy to invoke the DFU helper directly instead of asking an absent application to
/// enter the bootloader again.
pub fn observe_t114_serial_dfu_port(
    port: &str,
    revision: String,
    loader_snapshot: &linkboy::T114LoaderSnapshot,
) -> DeviceObservation {
    let facts = loader_snapshot.serial_dfu_observation();
    DeviceObservation::from_bootloader(
        linkboy::DeviceTransport::SerialDfuPort(port.to_string()),
        facts.clone(),
    )
    .confirm_board(linkboy::BoardFamily::T114, revision)
    .with_hardware(linkboy::HardwareFacts {
        processor: facts.processor.clone(),
        flash_size: facts.flash_size,
        bootloader: facts.bootloader.clone(),
        loader_route: Some("captured-t114-loader-snapshot".into()),
        bootloader_usb: Some(facts),
    })
}

/// Observe an explicitly named mounted T114 UF2 volume, retaining the record needed for a
/// later serial-DFU restore. The owner still supplies the board revision; the mounted volume
/// proves the loader profile but not a revision printed on the carrier.
pub fn observe_t114_uf2_volume(
    volume: &str,
    revision: String,
) -> Result<(DeviceObservation, linkboy::T114LoaderSnapshot), FirmwareError> {
    let (observation, snapshot) =
        linkboy::t114_uf2_observation(volume).map_err(FirmwareError::Discovery)?;
    Ok((
        observation.confirm_board(linkboy::BoardFamily::T114, revision),
        snapshot,
    ))
}

/// Capture the mounted bootloader record at the owner-selected path, then return the immutable
/// observation for the UF2 package plan. The desktop face asks Signalman to do this instead of
/// serializing Linkboy evidence itself.
pub fn capture_t114_uf2_volume(
    volume: &str,
    revision: String,
    record_path: impl AsRef<Path>,
) -> Result<DeviceObservation, FirmwareError> {
    let (observation, snapshot) = observe_t114_uf2_volume(volume, revision)?;
    snapshot
        .save_json(record_path)
        .map_err(|error| FirmwareError::LoaderSnapshot(error.to_string()))?;
    Ok(observation)
}

/// A refusal, as separate visible lines.
///
/// Every reason Linkboy structures is rendered; none is summarized away. A
/// refusal an owner cannot read is a disabled button with no explanation, which
/// the owner flow exists to avoid.
pub fn refusal_lines(error: &FlowError) -> Vec<String> {
    match error {
        FlowError::Refused(refusal) => refusal.reasons.iter().map(ToString::to_string).collect(),
        other => vec![other.to_string()],
    }
}

/// One owner-facing line for an execution event.
///
/// `None` for events a face shows structurally rather than as prose — the
/// terminal ones, which become the receipt or the recovery page.
pub fn describe_event(event: &FlashEvent) -> Option<String> {
    Some(match event {
        FlashEvent::Inspecting { device, package_id } => {
            format!("Inspecting {device} for {package_id}")
        }
        FlashEvent::WaitingForOwnerAction { message } => message.clone(),
        FlashEvent::EnteringBootloader => "Putting the board in its bootloader".into(),
        FlashEvent::Rediscovering => "Waiting for the board to come back".into(),
        FlashEvent::Erasing => "Erasing".into(),
        FlashEvent::Writing { written, total } => {
            let pct = if *total > 0 {
                (*written as f64 / *total as f64 * 100.0).round() as u32
            } else {
                0
            };
            format!("Writing {written} of {total} bytes ({pct}%)")
        }
        FlashEvent::VerifyingTransfer => "Verifying what was written".into(),
        FlashEvent::Rebooting => "Rebooting the board".into(),
        FlashEvent::VerifyingApplication => "Asking the board what it is now".into(),
        FlashEvent::Complete { .. }
        | FlashEvent::ManualCheckRequired { .. }
        | FlashEvent::RecoveryRequired { .. } => return None,
        FlashEvent::Refused { reasons } => {
            format!(
                "Refused: {}",
                reasons
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        }
    })
}

/// How far through the write an event puts the transfer, as `0.0..=1.0`.
/// `None` when the event carries no progress.
pub fn event_progress(event: &FlashEvent) -> Option<f32> {
    match event {
        FlashEvent::Writing { written, total } if *total > 0 => {
            Some((*written as f32 / *total as f32).clamp(0.0, 1.0))
        }
        _ => None,
    }
}

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

/// A host-neutral request to drain a product-owned worker. This is the same
/// callback shape Armillary actors use, without making Signalman depend on a
/// particular GUI or actor runtime.
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
/// Its Linkboy executor message is intentionally private. A face gives it
/// back to [`FirmwareInstaller::apply_install_update`], which advances the
/// owner flow and returns an owner-facing [`FirmwareInstallNotice`].
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
    /// A nonterminal status line, with write progress where the executor has
    /// one. `replaces_previous_progress` keeps a face's event log compact.
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
/// The worker owns only copies of Linkboy's already-approved inputs. The
/// caller owns the receiver and drains it after its own host wakes, so no DOM,
/// renderer, or application state crosses the thread boundary.
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
                    // Recovery emits its own terminal event with the facts the
                    // owner needs. Reporting it again would make a face show a
                    // duplicate failure.
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

    /// Start exactly the plan the owner flow approved. The desktop or another
    /// face supplies only a wake callback; Signalman owns the helper runners,
    /// blocking thread, and executor call.
    pub fn start_install(
        &mut self,
        wake: InstallerWake,
    ) -> Result<FirmwareInstallWorker, FirmwareError> {
        let (plan, package) = self.flow.begin_install()?;
        FirmwareInstallWorker::start(plan.clone(), package.clone(), wake)
            .map_err(FirmwareError::Worker)
    }

    /// Apply one private worker update to the owning flow and return only the
    /// presentation facts a face needs. This is the execution boundary: a GUI
    /// does not match Linkboy's executor protocol or manufacture a receipt.
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

    /// The finished receipt, once the flow has one.
    ///
    /// [`FirmwareView`] carries only the receipt's *result*, which is enough to
    /// decide a page but not to show what was actually written to which board.
    /// A face that retains the receipt as its own record needs the record.
    pub fn receipt(&self) -> Option<&linkboy::FlashReceipt> {
        self.flow.receipt()
    }

    /// The approved plan, once one exists. Shared, never owned: a face renders
    /// plan facts and cannot alter them.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_starts_with_the_first_owner_page() {
        let installer = FirmwareInstaller::new();
        let view = installer.view();
        assert_eq!(view.stage, OwnerStage::ChooseDevice);
        assert_eq!(view.title, "Choose device");
        assert_eq!(view.device, None);
        assert_eq!(view.review, None);
        assert_eq!(view.result, None);
    }

    #[test]
    fn catalog_loads_and_resolves_verified_packages() {
        let index_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../firmware/packages/index.toml");
        let catalog = FirmwareCatalog::load(index_path).expect("package catalog should load");
        assert!(
            catalog.packages().len() >= 2,
            "the Retinue V4 and T114 packages remain present alongside any added packages"
        );
        assert_eq!(
            catalog
                .package("retinue.heltec-v4")
                .expect("V4 should be catalogued")
                .state,
            linkboy::CatalogState::ProvenRecipe
        );
        let package = catalog
            .load_package("retinue.t114")
            .expect("T114 manifest should resolve");
        assert_eq!(package.manifest().package_id, "retinue.t114");
    }

    #[test]
    fn an_owner_confirmed_t114_dfu_port_keeps_loader_evidence_and_transport_state() {
        let snapshot = linkboy::T114LoaderSnapshot {
            schema: linkboy::discovery::T114_LOADER_SNAPSHOT_SCHEMA,
            model: "HT-n5262".into(),
            uf2_bootloader: "0.9.0".into(),
            softdevice: "S140 6.1.1".into(),
            processor: linkboy::ProcessorKind::Nrf52840,
            flash_size: 1024 * 1024,
        };

        let observation = observe_t114_serial_dfu_port("COM10", "2.x".into(), &snapshot);

        assert_eq!(
            observation.transport,
            linkboy::DeviceTransport::SerialDfuPort("COM10".into())
        );
        assert_eq!(
            observation.firmware,
            linkboy::device::FirmwareState::Bootloader
        );
        assert_eq!(
            observation.hardware.loader_route.as_deref(),
            Some("captured-t114-loader-snapshot")
        );
        assert_eq!(
            observation
                .selected_board
                .as_ref()
                .map(|board| &board.family),
            Some(&linkboy::BoardFamily::T114)
        );
    }
}
