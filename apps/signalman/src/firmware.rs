//! Signalman's projection of Linkboy's owner installer.
//!
//! This is deliberately semantic rather than terminal-shaped. A future window can render the
//! view with its own controls while Linkboy remains the only owner of package policy, plans, and
//! execution events.

use std::path::{Path, PathBuf};

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
    let found = linkboy::identify(port);
    let mut observation = DeviceObservation::from_found(&found);
    if matches!(found.board, Some(linkboy::Board::HeltecV4)) {
        let mut process = linkboy::SystemProcessRunner;
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
    if let Some((family, revision)) = selection {
        observation = observation.confirm_board(family, revision);
    }
    Ok(observation)
}

/// A refusal, as separate visible lines.
///
/// Every reason Linkboy structures is rendered; none is summarized away. A
/// refusal an owner cannot read is a disabled button with no explanation, which
/// the owner flow exists to avoid.
pub fn refusal_lines(error: &FlowError) -> Vec<String> {
    match error {
        FlowError::Refused(refusal) => {
            refusal.reasons.iter().map(ToString::to_string).collect()
        }
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
        FlashEvent::Complete { .. } | FlashEvent::RecoveryRequired { .. } => return None,
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
    pub payload_sha256: String,
    pub license: String,
    pub source_url: String,
    pub origin_url: String,
    pub board: String,
    pub board_revision: String,
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

#[derive(Debug)]
pub enum FirmwareError {
    Catalog(CatalogError),
    Flow(FlowError),
    /// A loader run that could not produce the facts a plan needs.
    Execution(linkboy::ExecutionError),
}

impl std::fmt::Display for FirmwareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Catalog(error) => error.fmt(formatter),
            Self::Flow(error) => error.fmt(formatter),
            Self::Execution(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FirmwareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Catalog(error) => Some(error),
            Self::Flow(error) => Some(error),
            Self::Execution(error) => Some(error),
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

fn review(plan: &FlashPlan, package: &FlashPackage) -> FirmwareReview {
    let manifest = package.manifest();
    let helper = manifest.helper_for(plan.route());
    FirmwareReview {
        package_id: manifest.package_id.clone(),
        display_name: manifest.display_name.clone(),
        version: manifest.version.clone(),
        publisher: manifest.publisher.clone(),
        payload_sha256: manifest.payload.sha256.clone(),
        license: manifest.license.clone(),
        source_url: manifest.source_url.clone(),
        origin_url: manifest.origin_url.clone(),
        board: plan.board().family.to_string(),
        board_revision: plan.board().revision.clone(),
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
        assert_eq!(catalog.packages().len(), 2);
        assert_eq!(
            catalog
                .package("retinue.heltec-v4")
                .expect("V4 should be catalogued")
                .state,
            linkboy::CatalogState::Partial
        );
        let package = catalog
            .load_package("retinue.t114")
            .expect("T114 manifest should resolve");
        assert_eq!(package.manifest().package_id, "retinue.t114");
    }
}
