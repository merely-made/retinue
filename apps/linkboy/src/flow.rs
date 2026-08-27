//! UI-neutral owner flow for a future graphical face.
//!
//! The flow owns page transitions and the approved plan. A renderer can observe the state and
//! feed executor events back here without reconstructing flashing policy or mutating the plan.

use crate::device::DeviceObservation;
use crate::executor::{FlashEvent, RecoveryFacts};
use crate::package::FlashPackage;
use crate::plan::{FlashPlan, Refusal, plan_flash};
use crate::receipt::FlashReceipt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerStage {
    ChooseDevice,
    ChooseFirmware,
    ReviewChanges,
    PrepareDevice,
    Install,
    VerifyOrRecover,
}

#[derive(Debug)]
pub struct OwnerFlow {
    stage: OwnerStage,
    observation: Option<DeviceObservation>,
    package: Option<FlashPackage>,
    plan: Option<FlashPlan>,
    receipt: Option<FlashReceipt>,
    recovery: Option<RecoveryFacts>,
}

impl Default for OwnerFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnerFlow {
    pub fn new() -> Self {
        Self {
            stage: OwnerStage::ChooseDevice,
            observation: None,
            package: None,
            plan: None,
            receipt: None,
            recovery: None,
        }
    }

    pub fn stage(&self) -> OwnerStage {
        self.stage
    }

    pub fn observation(&self) -> Option<&DeviceObservation> {
        self.observation.as_ref()
    }

    pub fn package(&self) -> Option<&FlashPackage> {
        self.package.as_ref()
    }

    /// The approved plan is immutable through this flow. A renderer receives a shared view and
    /// can only advance it through the explicit review and event methods below.
    pub fn approved_plan(&self) -> Option<&FlashPlan> {
        self.plan.as_ref()
    }

    pub fn receipt(&self) -> Option<&FlashReceipt> {
        self.receipt.as_ref()
    }

    pub fn recovery(&self) -> Option<&RecoveryFacts> {
        self.recovery.as_ref()
    }

    pub fn choose_device(&mut self, observation: DeviceObservation) -> Result<(), FlowError> {
        if self.stage != OwnerStage::ChooseDevice {
            return Err(FlowError::WrongStage {
                expected: OwnerStage::ChooseDevice,
                actual: self.stage,
            });
        }
        self.stage = OwnerStage::ChooseFirmware;
        self.observation = Some(observation);
        self.package = None;
        self.plan = None;
        self.receipt = None;
        self.recovery = None;
        Ok(())
    }

    pub fn choose_firmware(&mut self, package: FlashPackage) -> Result<(), FlowError> {
        if self.stage != OwnerStage::ChooseFirmware {
            return Err(FlowError::WrongStage {
                expected: OwnerStage::ChooseFirmware,
                actual: self.stage,
            });
        }
        let Some(observation) = self.observation.as_ref() else {
            return Err(FlowError::DeviceUnavailable);
        };
        let plan = plan_flash(observation, &package).map_err(FlowError::Refused)?;
        self.package = Some(package);
        self.plan = Some(plan);
        self.receipt = None;
        self.recovery = None;
        self.stage = OwnerStage::ReviewChanges;
        Ok(())
    }

    pub fn approve_changes(&mut self) -> Result<(), FlowError> {
        if self.plan.is_none() {
            return Err(FlowError::PlanUnavailable);
        }
        if self.stage != OwnerStage::ReviewChanges {
            return Err(FlowError::WrongStage {
                expected: OwnerStage::ReviewChanges,
                actual: self.stage,
            });
        }
        self.stage = OwnerStage::PrepareDevice;
        Ok(())
    }

    /// Move to installation and return the exact approved inputs for `execute_plan`.
    pub fn begin_install(&mut self) -> Result<(&FlashPlan, &FlashPackage), FlowError> {
        if self.stage != OwnerStage::PrepareDevice {
            return Err(FlowError::WrongStage {
                expected: OwnerStage::PrepareDevice,
                actual: self.stage,
            });
        }
        let (Some(plan), Some(package)) = (self.plan.as_ref(), self.package.as_ref()) else {
            return Err(FlowError::PlanUnavailable);
        };
        self.stage = OwnerStage::Install;
        Ok((plan, package))
    }

    /// Feed one executor event into the owner flow. Terminal events create the only state from
    /// which a UI may present completion or recovery actions.
    pub fn apply_event(&mut self, event: &FlashEvent) {
        match event {
            FlashEvent::Complete { receipt } => {
                self.receipt = Some(receipt.clone());
                self.recovery = None;
                self.stage = OwnerStage::VerifyOrRecover;
            }
            FlashEvent::ManualCheckRequired { receipt } => {
                self.receipt = Some(receipt.clone());
                self.recovery = None;
                self.stage = OwnerStage::VerifyOrRecover;
            }
            FlashEvent::RecoveryRequired { facts, receipt, .. } => {
                self.receipt = Some(receipt.clone());
                self.recovery = Some(facts.clone());
                self.stage = OwnerStage::VerifyOrRecover;
            }
            FlashEvent::Refused { .. } => {}
            _ if self.stage == OwnerStage::PrepareDevice => {
                self.stage = OwnerStage::Install;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    #[error("a selected device is required")]
    DeviceUnavailable,
    #[error("a reviewed flash plan is required")]
    PlanUnavailable,
    #[error("flow is on {actual:?}; expected {expected:?}")]
    WrongStage {
        expected: OwnerStage,
        actual: OwnerStage,
    },
    #[error(transparent)]
    Refused(#[from] Refusal),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{
        BoardSelection, DeviceTransport, EvidenceConfidence, FirmwareState, HardwareFacts,
    };
    use crate::package::{
        BoardFamily, ExpectedApplication, FlashPackageManifest, FlashRange, HelperRequirement,
        PACKAGE_SCHEMA, PackagePayload, PackageTarget, PayloadFormat, RecoveryInstructions,
        StateImpact,
    };
    use crate::receipt::ApplicationVerification;

    fn observation() -> DeviceObservation {
        DeviceObservation {
            transport: DeviceTransport::SerialPort("COM7".into()),
            status_reply: Some("tulle/heltec-v4 phy online; version=0.0.1".into()),
            hardware: HardwareFacts {
                processor: Some(crate::package::ProcessorKind::Esp32S3),
                flash_size: Some(4 * 1024 * 1024),
                bootloader: Some("esp-rom".into()),
                loader_route: Some("esp-rom".into()),
                bootloader_usb: None,
            },
            selected_board: Some(BoardSelection::owner_confirmed(
                BoardFamily::HeltecV4,
                "4.2",
            )),
            firmware: FirmwareState::Retinue {
                family: BoardFamily::HeltecV4,
            },
            confidence: EvidenceConfidence::OwnerConfirmed,
            contradictions: Vec::new(),
            native_node_state: crate::device::NativeNodeState::Unknown,
        }
    }

    fn package() -> FlashPackage {
        let bytes = b"flow-payload".to_vec();
        let manifest = FlashPackageManifest {
            schema: PACKAGE_SCHEMA,
            package_id: "flow.test".into(),
            display_name: "Flow test".into(),
            version: "1".into(),
            publisher: "Test".into(),
            helpers: vec![HelperRequirement {
                route: crate::package::FlashRoute::EspRom,
                program: "espflash".into(),
                version: "4.5.0".into(),
                binary_sha256: None,
                artifacts: Vec::new(),
                license: "MIT OR Apache-2.0".into(),
                source_url: "https://example.invalid/espflash".into(),
                notice: "Test".into(),
            }],
            payload: Some(PackagePayload {
                path: "payload".into(),
                format: PayloadFormat::EspflashElf,
                byte_length: bytes.len() as u64,
                sha256: crate::package::sha256_hex(&bytes),
                write_bytes: bytes.len() as u64,
            }),
            parts: Vec::new(),
            targets: vec![PackageTarget {
                family: BoardFamily::HeltecV4,
                revision: "4.2".into(),
                processor: crate::package::ProcessorKind::Esp32S3,
                flash_size: 4 * 1024 * 1024,
                bootloader: "esp-rom".into(),
                route: crate::package::FlashRoute::EspRom,
            }],
            write_ranges: vec![FlashRange {
                start: 0,
                length: 1,
            }],
            preserved_ranges: vec![FlashRange {
                start: 1,
                length: 1,
            }],
            regions: vec!["US915".into()],
            channel_capabilities: vec!["modem".into(), "rnode".into()],
            state_impact: StateImpact::Preserved,
            expected_application: ExpectedApplication {
                board: BoardFamily::HeltecV4,
                version: "0.0.1".into(),
                manual_check: None,
            },
            license: "MPL-2.0".into(),
            notices: "Test".into(),
            source_revision: "test".into(),
            source_url: "https://example.invalid/source".into(),
            origin_url: "https://example.invalid/package".into(),
            publisher_signature: None,
            recovery: RecoveryInstructions {
                before_write: "Keep cable attached.".into(),
                after_failure: "Use the ROM loader.".into(),
            },
            persistent_state: None,
        };
        FlashPackage::from_parts(manifest, "manifest", "payload", bytes).unwrap()
    }

    #[test]
    fn owner_flow_keeps_the_approved_plan_inside_the_engine() {
        let mut flow = OwnerFlow::new();
        assert_eq!(flow.stage(), OwnerStage::ChooseDevice);
        flow.choose_device(observation()).unwrap();
        assert_eq!(flow.stage(), OwnerStage::ChooseFirmware);
        flow.choose_firmware(package()).unwrap();
        assert_eq!(flow.stage(), OwnerStage::ReviewChanges);
        assert!(flow.approved_plan().is_some());
        flow.approve_changes().unwrap();
        let identities = {
            let (plan, package) = flow.begin_install().unwrap();
            (
                plan.package().package_id.clone(),
                package.manifest().package_id.clone(),
            )
        };
        assert_eq!(flow.stage(), OwnerStage::Install);
        assert_eq!(identities.0, identities.1);
    }

    #[test]
    fn terminal_events_move_the_flow_to_verify_or_recover() {
        let mut flow = OwnerFlow::new();
        flow.choose_device(observation()).unwrap();
        flow.choose_firmware(package()).unwrap();
        flow.approve_changes().unwrap();
        flow.begin_install().unwrap();
        let receipt = FlashReceipt::complete(
            flow.approved_plan().unwrap(),
            ApplicationVerification {
                board: BoardFamily::HeltecV4,
                version: "0.0.1".into(),
                region: Some("US915".into()),
                channel: Some("rnode".into()),
            },
            vec![],
        );
        flow.apply_event(&FlashEvent::Complete {
            receipt: receipt.clone(),
        });
        assert_eq!(flow.stage(), OwnerStage::VerifyOrRecover);
        assert_eq!(flow.receipt(), Some(&receipt));
    }
}
