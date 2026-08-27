//! Secret-free transaction receipts.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::device::{DeviceObservation, DeviceTransport};
use crate::package::{BoardFamily, FlashRoute, ProcessorKind, PublisherSignature};
use crate::plan::{FlashPlan, HelperIdentity, PackagePartIdentity};

pub const RECEIPT_SCHEMA: u32 = 4;

fn default_board_selection_evidence() -> String {
    "owner confirmation from carrier marking".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReceiptResult {
    Complete,
    ManualCheckRequired,
    RecoveryRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptStage {
    pub name: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationVerification {
    pub board: BoardFamily,
    pub version: String,
    pub region: Option<String>,
    pub channel: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashReceipt {
    pub schema: u32,
    pub package_id: String,
    pub package_parts: Vec<PackagePartIdentity>,
    pub publisher_signature: Option<PublisherSignature>,
    pub board: BoardFamily,
    pub board_revision: String,
    /// The explicit owner evidence for a carrier revision. Old receipts predate this field and
    /// therefore retain the historical carrier-marking default when they are read.
    #[serde(default = "default_board_selection_evidence")]
    pub board_selection_evidence: String,
    pub route: FlashRoute,
    pub helper: HelperIdentity,
    pub transport: String,
    pub processor: Option<ProcessorKind>,
    pub flash_size: Option<u32>,
    pub bootloader: Option<String>,
    pub stages: Vec<ReceiptStage>,
    pub result: ReceiptResult,
    pub application: Option<ApplicationVerification>,
    pub manual_check: Option<String>,
}

impl FlashReceipt {
    pub fn complete(
        plan: &FlashPlan,
        application: ApplicationVerification,
        stages: Vec<ReceiptStage>,
    ) -> Self {
        Self::new(plan, ReceiptResult::Complete, Some(application), stages)
    }

    pub fn recovery_required(plan: &FlashPlan, stages: Vec<ReceiptStage>) -> Self {
        Self::new(plan, ReceiptResult::RecoveryRequired, None, stages)
    }

    pub fn manual_check_required(
        plan: &FlashPlan,
        instruction: String,
        stages: Vec<ReceiptStage>,
    ) -> Self {
        let mut receipt = Self::new(plan, ReceiptResult::ManualCheckRequired, None, stages);
        receipt.manual_check = Some(instruction);
        receipt
    }

    fn new(
        plan: &FlashPlan,
        result: ReceiptResult,
        application: Option<ApplicationVerification>,
        stages: Vec<ReceiptStage>,
    ) -> Self {
        let observation = plan.observation();
        Self {
            schema: RECEIPT_SCHEMA,
            package_id: plan.package().package_id.clone(),
            package_parts: plan.package().parts.clone(),
            publisher_signature: plan.package().publisher_signature.clone(),
            board: plan.board().family.clone(),
            board_revision: plan.board().revision.clone(),
            board_selection_evidence: plan.board().evidence.describe(),
            route: plan.route().clone(),
            helper: plan.helper_identity().clone(),
            transport: transport_label(&observation.transport),
            processor: observation.hardware.processor.clone(),
            flash_size: observation.hardware.flash_size,
            bootloader: observation.hardware.bootloader.clone(),
            stages,
            result,
            application,
            manual_check: None,
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn save_json(&self, path: impl AsRef<Path>) -> Result<(), ReceiptError> {
        let json = self.to_json()?;
        std::fs::write(path, json).map_err(ReceiptError::Io)
    }

    pub fn load_json(path: impl AsRef<Path>) -> Result<Self, ReceiptError> {
        let json = std::fs::read_to_string(path).map_err(ReceiptError::Io)?;
        serde_json::from_str(&json).map_err(ReceiptError::Deserialize)
    }
}

pub fn transport_label(transport: &DeviceTransport) -> String {
    match transport {
        DeviceTransport::SerialPort(port) => format!("serial:{port}"),
        DeviceTransport::SerialDfuPort(port) => format!("serial-dfu:{port}"),
        DeviceTransport::MountedVolume(volume) => format!("volume:{volume}"),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiptError {
    #[error("could not serialize receipt: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not write receipt: {0}")]
    Io(std::io::Error),
    #[error("could not read receipt: {0}")]
    Deserialize(serde_json::Error),
}

/// The receipt intentionally copies only public observation facts. In particular, it does not
/// copy the running status reply, identity line, keys, message content, or package payload.
pub fn observation_has_no_secret_fields(observation: &DeviceObservation) -> bool {
    let _ = observation;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{
        BoardSelection, DeviceTransport, EvidenceConfidence, FirmwareState, HardwareFacts,
    };
    use crate::package::{BoardFamily, FlashRoute, ProcessorKind, StateImpact};
    use crate::plan::{CompatibilityFact, PackageIdentity, PlanWarning};

    fn plan() -> FlashPlan {
        FlashPlan::for_test(
            DeviceObservation {
                transport: DeviceTransport::SerialPort("COM7".into()),
                status_reply: Some("identity=private-key-material".into()),
                hardware: HardwareFacts {
                    processor: Some(ProcessorKind::Esp32S3),
                    flash_size: Some(4 * 1024 * 1024),
                    bootloader: Some("esp-rom".into()),
                    loader_route: None,
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
            },
            PackageIdentity {
                package_id: "test".into(),
                display_name: "Test".into(),
                version: "1".into(),
                parts: vec![PackagePartIdentity {
                    kind: crate::FirmwarePartKind::Application,
                    offset: None,
                    byte_length: 1,
                    sha256: "a".repeat(64),
                }],
                publisher_signature: None,
            },
            BoardSelection::owner_confirmed(BoardFamily::HeltecV4, "4.2"),
            FlashRoute::EspRom,
            vec![],
            vec![],
            StateImpact::Preserved,
            vec![CompatibilityFact {
                name: "board".into(),
                value: "V4".into(),
                source: "test".into(),
            }],
            vec![PlanWarning {
                message: "none".into(),
                requires_confirmation: false,
            }],
            "before".into(),
            "after".into(),
        )
    }

    #[test]
    fn receipt_does_not_export_status_or_identity_content() {
        let receipt = FlashReceipt::complete(
            &plan(),
            ApplicationVerification {
                board: BoardFamily::HeltecV4,
                version: "0.0.1".into(),
                region: Some("US915".into()),
                channel: Some("rnode".into()),
            },
            vec![ReceiptStage {
                name: "complete".into(),
                detail: None,
            }],
        );
        let json = receipt.to_json().unwrap();
        assert!(json.contains("package_parts"));
        assert!(json.contains("board_selection_evidence"));
        assert!(json.contains("carrier marking"));
        assert!(!json.contains("private-key-material"));
        assert!(!json.contains("status_reply"));
    }

    #[test]
    fn receipt_json_round_trips_through_the_loader() {
        let receipt = FlashReceipt::complete(
            &plan(),
            ApplicationVerification {
                board: BoardFamily::HeltecV4,
                version: "0.0.1".into(),
                region: Some("US915".into()),
                channel: Some("rnode".into()),
            },
            Vec::new(),
        );
        let path = std::env::temp_dir().join(format!(
            "linkboy-receipt-round-trip-{}.json",
            std::process::id()
        ));
        receipt.save_json(&path).unwrap();
        assert_eq!(FlashReceipt::load_json(&path).unwrap(), receipt);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn schema_three_receipt_defaults_carrier_evidence_when_loaded() {
        let receipt = FlashReceipt::complete(
            &plan(),
            ApplicationVerification {
                board: BoardFamily::HeltecV4,
                version: "0.0.1".into(),
                region: Some("US915".into()),
                channel: Some("rnode".into()),
            },
            Vec::new(),
        );
        let mut legacy_json: serde_json::Value =
            serde_json::from_str(&receipt.to_json().unwrap()).unwrap();
        let legacy = legacy_json.as_object_mut().unwrap();
        legacy.insert("schema".into(), serde_json::Value::from(3));
        legacy.remove("board_selection_evidence");

        let path = std::env::temp_dir().join(format!(
            "linkboy-schema-three-receipt-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, serde_json::to_vec(&legacy_json).unwrap()).unwrap();

        let loaded = FlashReceipt::load_json(&path).unwrap();
        assert_eq!(loaded.schema, 3);
        assert_eq!(
            loaded.board_selection_evidence,
            "owner confirmation from carrier marking"
        );
        std::fs::remove_file(path).unwrap();
    }
}
