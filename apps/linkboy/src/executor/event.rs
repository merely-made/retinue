//! Execution stages, events, errors, and the recovery path.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::device::DeviceTransport;
use crate::package::RecoveryInstructions;
use crate::plan::{FlashPlan, RefusalReason};
use crate::receipt::{FlashReceipt, ReceiptStage};

use super::{DeviceFailure, ProcessFailure};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionStage {
    Preparing,
    EnteringBootloader,
    Transfer,
    Rebooting,
    VerifyingApplication,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryFacts {
    pub stage: ExecutionStage,
    pub transport: String,
    pub last_known_port: Option<String>,
    pub write_started: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlashEvent {
    Inspecting {
        device: String,
        package_id: String,
    },
    WaitingForOwnerAction {
        message: String,
    },
    EnteringBootloader,
    Rediscovering,
    Erasing,
    Writing {
        written: u64,
        total: u64,
    },
    VerifyingTransfer,
    Rebooting,
    VerifyingApplication,
    Complete {
        receipt: FlashReceipt,
    },
    ManualCheckRequired {
        receipt: FlashReceipt,
    },
    RecoveryRequired {
        facts: RecoveryFacts,
        instructions: RecoveryInstructions,
        receipt: FlashReceipt,
    },
    Refused {
        reasons: Vec<RefusalReason>,
    },
}

// RecoveryRequired deliberately carries the full recovery facts, instructions and receipt
// inline: it is the cold path, and the operator-facing surfaces need all of it at once.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExecutionError {
    #[error("execution requires a serial device")]
    UnsupportedTransport,
    #[error("cannot write UF2 volume {volume}: {detail}")]
    VolumeWrite { volume: String, detail: String },
    #[error(
        "this executor cannot write the approved multi-part package; no device was opened or changed"
    )]
    UnsupportedPackageLayout,
    #[error("process failed: {0}")]
    Process(#[from] ProcessFailure),
    #[error("device failed: {0}")]
    Device(#[from] DeviceFailure),
    #[error("recovery required: {detail}")]
    RecoveryRequired {
        facts: RecoveryFacts,
        instructions: RecoveryInstructions,
        detail: String,
        receipt: FlashReceipt,
    },
}

pub(super) fn recover(
    plan: &FlashPlan,
    emit: &mut dyn FnMut(FlashEvent),
    stage: ExecutionStage,
    port: &str,
    write_started: bool,
    detail: String,
) -> ExecutionError {
    let facts = RecoveryFacts {
        stage: stage.clone(),
        transport: match &plan.observation().transport {
            DeviceTransport::SerialPort(port) => format!("serial:{port}"),
            DeviceTransport::SerialDfuPort(port) => format!("serial-dfu:{port}"),
            DeviceTransport::MountedVolume(volume) => format!("volume:{volume}"),
        },
        last_known_port: Some(port.to_string()),
        write_started,
        detail: detail.clone(),
    };
    let instructions = RecoveryInstructions {
        before_write: plan.recovery_before_write().into(),
        after_failure: plan.recovery_after_failure().into(),
    };
    let receipt = FlashReceipt::recovery_required(
        plan,
        vec![ReceiptStage {
            name: format!("recovery-{stage:?}"),
            detail: Some(detail.clone()),
        }],
    );
    emit(FlashEvent::RecoveryRequired {
        facts: facts.clone(),
        instructions: instructions.clone(),
        receipt: receipt.clone(),
    });
    ExecutionError::RecoveryRequired {
        facts,
        instructions,
        detail,
        receipt,
    }
}
