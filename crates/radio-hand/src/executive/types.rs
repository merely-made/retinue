//! The seams a board implements and the reports the executive returns.

use lora_phy::mod_params::{ModulationParams, PacketParams};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa};
use radio_face::{HostSnapshot, LedSignal, LocalStatus};
use selvage::PhyProfile;

/// The radio settings a profile owns, held here so a rejected profile cannot half-apply:
/// [`crate::service::apply_profile`] builds a replacement and only a complete one is swapped
/// in.
pub struct RadioState {
    /// The complete profile represented by the driver parameters below.
    pub profile: PhyProfile,
    pub modulation: ModulationParams,
    pub tx: PacketParams,
    pub rx: PacketParams,
    pub tx_power_dbm: i32,
    /// Set when the radio must be put back into receive before the next wait.
    pub prepare_rx: bool,
}

/// The board's local face. Function pointers, since both images expose these as free
/// functions and no borrow of UI state is needed.
pub struct Face {
    pub publish: fn(LocalStatus, LedSignal),
    pub publish_host: fn(HostSnapshot),
}

/// A board's ability to read its radio chip's diagnostic registers.
///
/// Chip-specific (`sx126x_diagnostics` lives on the SX126x kind), so it borrows the
/// executive's `lora` for the length of the call.
///
/// Ordering is load-bearing: the host attaches the most recent diagnostic to a failed
/// transmit, so one emitted after its `EVENT_TX` reply would be misattributed.
/// No `Send` bound: the board's single-threaded executor never sends these futures, and a
/// bound would exclude the HAL types that implement this.
#[allow(async_fn_in_trait)]
pub trait ChipDiagnostics<RK: RadioKind, DLY: DelayNs> {
    /// The seven-byte `EVENT_DIAGNOSTIC` body, including its marker.
    async fn read(&self, lora: &mut LoRa<RK, DLY>) -> [u8; 7];
}

/// The board's own persistent facts and its entropy.
///
/// One trait because one object holds both on the T114 (NVMC pages and hardware RNG). It
/// lives on the executive per structural decision 4, so a channel cannot reach the store.
pub trait BoardStore {
    /// Fill `out` with random bytes.
    ///
    /// Fallible on purpose: a board may have no entropy source, and filling with zeros would
    /// be silently worse than refusing.
    fn random(&mut self, out: &mut [u8]) -> Result<(), StoreFault>;

    /// Persist new settings, keeping the identity already stored.
    ///
    /// Erase stalls the CPU and blanks receive, so a caller either runs before the radio
    /// starts or resets immediately after (pressure point 3).
    fn save(&mut self, settings: &crate::settings::Settings) -> Result<(), StoreFault>;
}

/// Why the board's store could not do what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFault {
    /// This board has no such facility. Not an error in the store; an absence of one.
    Unavailable,
    /// The write did not land, or did not read back.
    Write,
}

/// A board with neither persistence nor entropy.
///
/// The V4's state. It refuses rather than pretending, so a channel that needs entropy fails
/// loudly instead of announcing itself with zeros.
pub struct NoStore;

impl BoardStore for NoStore {
    fn random(&mut self, _out: &mut [u8]) -> Result<(), StoreFault> {
        Err(StoreFault::Unavailable)
    }

    fn save(&mut self, _settings: &crate::settings::Settings) -> Result<(), StoreFault> {
        Err(StoreFault::Unavailable)
    }
}

/// A received frame's length and signal report.
pub struct Received {
    pub len: usize,
    pub rssi: i16,
    pub snr: i16,
}

/// The radio would not do what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioFault;

/// Timings and outcome for one eight-symbol CAD observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CadObservation {
    /// Profile decode and sync-word application.
    pub apply_us: u64,
    /// Modem setup and carrier retune before CAD begins.
    pub retune_us: u64,
    /// The eight-symbol CAD operation itself.
    pub cad_us: u64,
    pub activity: bool,
}

/// Timings for entering one exact receive window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureArm {
    /// Profile decode and sync-word application.
    pub apply_us: u64,
    /// Continuous-RX setup after the exact profile was applied.
    pub handoff_us: u64,
}

/// What the executive has actually done with the radio, counted.
///
/// These counters expose a silently dead path on a board with no console; the `air` probe
/// prints them.
#[derive(Debug, Clone, Copy, Default)]
pub struct AirDiag {
    /// Times `ensure_rx` re-armed the receiver.
    pub rx_armed: u16,
    /// Times `ensure_rx` failed. Nonzero means the radio refused receive setup.
    pub rx_arm_failed: u16,
    /// Frames `receive` returned.
    pub rx_ok: u16,
    /// Times `receive` returned a fault.
    pub rx_err: u16,
    /// Packets that arrived, failed their CRC, and were dropped.
    ///
    /// Air, not fault; a count climbing faster than `rx_ok` is a link that needs a slower
    /// profile.
    pub rx_damaged: u16,
    /// Transmissions accepted on the air.
    pub tx_ok: u16,
    /// Transmissions refused or timed out.
    pub tx_err: u16,
    /// Transmissions refused because no region is configured. The regulatory floor
    /// working, not a fault.
    pub tx_no_region: u16,
    /// Transmissions refused because the region's duty budget was spent.
    pub tx_over_duty: u16,
    /// Listen-before-talk checks that found the channel clear on the first look.
    pub cad_clear: u16,
    /// Listen-before-talk checks that found activity and backed off.
    pub cad_busy: u16,
    /// Transmissions refused because the channel stayed busy where a region mandates
    /// carrier sense.
    pub tx_channel_busy: u16,
    /// Transmissions that took their turn after deferring for the whole courtesy budget.
    /// Nonzero means the band is contended, not that anything is wrong.
    pub cad_override: u16,
    /// Times the radio could not perform a listen check at all. Counted rather than fatal:
    /// see `listen_before_talk` for why this fails open.
    pub cad_fault: u16,
    /// Times the unattended wait woke for its heartbeat.
    pub wait_beats: u16,
    /// Times the unattended wait woke for a received frame.
    pub wait_frames: u16,
    /// CAD activity observations by stable DetectionProfile id (1 through 4).
    pub scan_cad_hits: [u16; 4],
    /// Empty CAD observations by stable DetectionProfile id (1 through 4).
    pub scan_cad_misses: [u16; 4],
    /// Captures by stable ReceiveProfile id (1 through 4).
    pub scan_rx_captures: [u16; 4],
    /// Empty capture windows by stable ReceiveProfile id (1 through 4).
    pub scan_rx_misses: [u16; 4],
}
