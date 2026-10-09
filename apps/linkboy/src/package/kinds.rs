//! Board, processor, route, and part vocabulary plus flash ranges.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A carrier-board family, not a processor family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardFamily {
    T114,
    HeltecV4,
}

impl BoardFamily {
    pub fn label(&self) -> &'static str {
        match self {
            Self::T114 => "T114",
            Self::HeltecV4 => "Heltec V4",
        }
    }

    pub fn from_board(board: &crate::Board) -> Option<Self> {
        match board {
            crate::Board::T114 => Some(Self::T114),
            crate::Board::HeltecV4 => Some(Self::HeltecV4),
            crate::Board::Unknown(_) => None,
        }
    }
}

impl fmt::Display for BoardFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Processor identities that a loader can prove for the first two routes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessorKind {
    Nrf52840,
    Esp32S3,
}

impl fmt::Display for ProcessorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Nrf52840 => "nRF52840",
            Self::Esp32S3 => "ESP32-S3",
        })
    }
}

/// The concrete transport route an executor will use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlashRoute {
    AdafruitDfu,
    EspRom,
    Uf2MassStorage,
}

impl FlashRoute {
    pub fn helper(&self) -> &'static str {
        match self {
            Self::AdafruitDfu => "adafruit-nrfutil",
            Self::EspRom => "espflash",
            Self::Uf2MassStorage => "linkboy UF2 volume writer",
        }
    }

    /// UF2 is a file-copy protocol implemented here, not an unpinned shell helper.
    pub fn uses_builtin_writer(&self) -> bool {
        matches!(self, Self::Uf2MassStorage)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::AdafruitDfu => "serial DFU (adafruit-nrfutil)",
            Self::EspRom => "ESP ROM loader (espflash)",
            Self::Uf2MassStorage => "UF2 mass-storage bootloader",
        }
    }
}

impl fmt::Display for FlashRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PayloadFormat {
    NrfDfuZip,
    EspflashElf,
    Uf2,
    RawBinary,
}

/// The job a part performs in a sparse image. The manifest order is also the write order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwarePartKind {
    Bootloader,
    PartitionTable,
    Application,
}

impl fmt::Display for FirmwarePartKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bootloader => "bootloader",
            Self::PartitionTable => "partition table",
            Self::Application => "application",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateImpact {
    Preserved,
    Replaced,
    Unknown,
}

impl fmt::Display for StateImpact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Preserved => "preserved",
            Self::Replaced => "replaced",
            Self::Unknown => "unknown",
        })
    }
}

/// A half-open byte range in target flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashRange {
    pub start: u32,
    pub length: u32,
}

impl FlashRange {
    pub fn end(&self) -> Option<u32> {
        self.start.checked_add(self.length)
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        let Some(end) = self.end() else {
            return true;
        };
        let Some(other_end) = other.end() else {
            return true;
        };
        self.start < other_end && other.start < end
    }
}
