//! Device facts and their provenance.
//!
//! A serial path is transport location only. The observation keeps it alongside status and
//! loader facts so later code can refuse a contradiction instead of promoting a convenient
//! operating-system label into a hardware identity.

use serde::{Deserialize, Serialize};

use crate::package::{BoardFamily, ProcessorKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceTransport {
    SerialPort(String),
    /// An owner-selected serial path already running the captured board's DFU loader.
    /// This is distinct from an application port because execution must not try to enter the
    /// bootloader again before invoking the serial-DFU helper.
    SerialDfuPort(String),
    MountedVolume(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwareState {
    Retinue { family: BoardFamily },
    Upstream { name: String },
    Bootloader,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceConfidence {
    Unknown,
    FamilyOnly,
    OwnerConfirmed,
    Contradictory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardSelection {
    pub family: BoardFamily,
    pub revision: String,
    pub confirmed_by_owner: bool,
    /// Why the owner may make this otherwise-unobservable carrier revision claim.
    ///
    /// This is deliberately recorded with the immutable plan: a USB identifier, COM port, or
    /// processor fact remains insufficient to establish a carrier revision.
    #[serde(default)]
    pub evidence: BoardSelectionEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BoardSelectionEvidence {
    /// The owner read the revision from the carrier itself.
    #[default]
    CarrierMarking,
    /// The owner identified a documented product profile whose manufacturer documentation names
    /// the carrier revision. This is narrower than a family-wide compatibility claim.
    DocumentedProductProfile {
        product: String,
        documentation_url: String,
    },
}

impl BoardSelectionEvidence {
    pub fn describe(&self) -> String {
        match self {
            Self::CarrierMarking => "owner confirmation from carrier marking".into(),
            Self::DocumentedProductProfile {
                product,
                documentation_url,
            } => format!(
                "owner confirmation from documented {product} profile ({documentation_url})"
            ),
        }
    }
}

impl BoardSelection {
    pub fn owner_confirmed(family: BoardFamily, revision: impl Into<String>) -> Self {
        Self {
            family,
            revision: revision.into(),
            confirmed_by_owner: true,
            evidence: BoardSelectionEvidence::CarrierMarking,
        }
    }

    pub fn documented_product_profile(
        family: BoardFamily,
        revision: impl Into<String>,
        product: impl Into<String>,
        documentation_url: impl Into<String>,
    ) -> Self {
        Self {
            family,
            revision: revision.into(),
            confirmed_by_owner: true,
            evidence: BoardSelectionEvidence::DocumentedProductProfile {
                product: product.into(),
                documentation_url: documentation_url.into(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BootloaderObservation {
    pub identifier: Option<String>,
    pub descriptor: Option<String>,
    pub processor: Option<ProcessorKind>,
    pub flash_size: Option<u32>,
    pub bootloader: Option<String>,
    pub usb_vid: Option<u16>,
    pub usb_pid: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HardwareFacts {
    pub processor: Option<ProcessorKind>,
    pub flash_size: Option<u32>,
    pub bootloader: Option<String>,
    pub loader_route: Option<String>,
    pub bootloader_usb: Option<BootloaderObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceObservation {
    pub transport: DeviceTransport,
    pub status_reply: Option<String>,
    pub hardware: HardwareFacts,
    pub selected_board: Option<BoardSelection>,
    pub firmware: FirmwareState,
    pub confidence: EvidenceConfidence,
    pub contradictions: Vec<String>,
}

impl DeviceObservation {
    pub fn from_bootloader(transport: DeviceTransport, bootloader: BootloaderObservation) -> Self {
        Self {
            transport,
            status_reply: None,
            hardware: HardwareFacts {
                processor: bootloader.processor.clone(),
                flash_size: bootloader.flash_size,
                bootloader: bootloader.bootloader.clone(),
                loader_route: None,
                bootloader_usb: Some(bootloader),
            },
            selected_board: None,
            firmware: FirmwareState::Bootloader,
            confidence: EvidenceConfidence::Unknown,
            contradictions: Vec::new(),
        }
    }

    pub fn from_found(found: &crate::Found) -> Self {
        let (firmware, confidence) = match &found.board {
            Some(board) => match BoardFamily::from_board(board) {
                Some(family) => (
                    FirmwareState::Retinue { family },
                    EvidenceConfidence::FamilyOnly,
                ),
                None => (FirmwareState::Unknown, EvidenceConfidence::Unknown),
            },
            None => (FirmwareState::Unknown, EvidenceConfidence::Unknown),
        };
        Self {
            transport: DeviceTransport::SerialPort(found.port.clone()),
            status_reply: (!found.banner.is_empty()).then(|| found.banner.clone()),
            hardware: HardwareFacts::default(),
            selected_board: None,
            firmware,
            confidence,
            contradictions: Vec::new(),
        }
    }

    pub fn confirm_board(self, family: BoardFamily, revision: impl Into<String>) -> Self {
        self.confirm_board_selection(BoardSelection::owner_confirmed(family, revision))
    }

    /// Attach a board selection that the owner has affirmatively made from either a carrier
    /// marking or a named product profile. Hardware facts can still contradict it and planning
    /// will still refuse them.
    pub fn confirm_board_selection(mut self, selection: BoardSelection) -> Self {
        let family = selection.family.clone();
        self.selected_board = Some(selection);
        self.confidence = EvidenceConfidence::OwnerConfirmed;
        if let Some(expected_processor) = expected_processor(&family) {
            if self.hardware.processor.as_ref() != Some(&expected_processor) {
                if let Some(observed) = &self.hardware.processor {
                    self.contradictions.push(format!(
                        "selected {} requires {}, loader reported {}",
                        family, expected_processor, observed
                    ));
                }
            }
        }
        self
    }

    pub fn with_hardware(mut self, hardware: HardwareFacts) -> Self {
        self.hardware = hardware;
        self
    }

    pub fn with_contradiction(mut self, contradiction: impl Into<String>) -> Self {
        self.contradictions.push(contradiction.into());
        self.confidence = EvidenceConfidence::Contradictory;
        self
    }
}

fn expected_processor(family: &BoardFamily) -> Option<ProcessorKind> {
    match family {
        BoardFamily::T114 => Some(ProcessorKind::Nrf52840),
        BoardFamily::HeltecV4 => Some(ProcessorKind::Esp32S3),
    }
}
