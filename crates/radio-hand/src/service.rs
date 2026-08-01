//! Driving the radio: the work both firmware images did identically.
//!
//! Applying a host profile is the same sequence on any SX126x board — decode, map the wire
//! values, build modulation and packet parameters, set the sync word, and only then commit
//! the new state. Both images spelled it out in full, sixty lines of nesting apiece, and
//! reported the outcome as a bare number.
//!
//! Board specifics stay out: this is generic over `RadioKind`, so the T114's software SPI
//! and the V4's `GenericSx126xInterfaceVariant` both satisfy it without this module knowing
//! either exists.

use lora_phy::mod_params::{ModulationParams, PacketParams};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa};
use selvage::{
    CONFIG_ACCEPTED, CONFIG_MALFORMED, CONFIG_RADIO_FAULT, CONFIG_UNSUPPORTED, PhyProfile,
};

use crate::phy::{bandwidth, coding_rate, spreading_factor};

/// Radio state a successfully applied profile produces.
///
/// The caller swaps its own copies in only on success, which is what keeps a rejected
/// profile from half-applying: nothing here exists unless every step passed.
pub struct Applied {
    pub modulation: ModulationParams,
    pub tx: PacketParams,
    pub rx: PacketParams,
    /// Transmit power, widened to the `i32` the driver's send path takes.
    pub tx_power_dbm: i32,
}

/// The largest receive payload the packet parameters allow. The SX126x maximum, and what
/// both images passed.
const MAX_RX_PAYLOAD: u8 = 255;

/// Apply a decoded profile to the radio.
///
/// Returns [`Applied`] for the caller to commit, or the [`selvage`] result code to report.
/// Nothing is committed here: the radio's sync word is the only device state this touches,
/// and it is set last, once every parameter has been accepted.
pub async fn apply_profile<RK, DLY>(
    lora: &mut LoRa<RK, DLY>,
    profile: &PhyProfile,
) -> Result<Applied, u8>
where
    RK: RadioKind,
    DLY: DelayNs,
{
    let Some(((sf, bw), cr)) = spreading_factor(profile.spreading_factor)
        .zip(bandwidth(profile.bandwidth_hz))
        .zip(coding_rate(profile.coding_rate_denominator))
    else {
        return Err(CONFIG_UNSUPPORTED);
    };

    let modulation = lora
        .create_modulation_params(sf, bw, cr, profile.frequency_hz)
        .map_err(|_| CONFIG_UNSUPPORTED)?;

    // The wire carries "explicit header" the way the protocol talks about it; the driver
    // takes the inverse. Getting this backwards costs an on-air incompatibility that looks
    // like a dead radio, so it is inverted in exactly one place now rather than two.
    let implicit_header = !profile.explicit_header;
    let tx = lora
        .create_tx_packet_params(
            profile.preamble_symbols,
            implicit_header,
            profile.crc,
            profile.invert_iq,
            &modulation,
        )
        .map_err(|_| CONFIG_UNSUPPORTED)?;
    let rx = lora
        .create_rx_packet_params(
            profile.preamble_symbols,
            implicit_header,
            MAX_RX_PAYLOAD,
            profile.crc,
            profile.invert_iq,
            &modulation,
        )
        .map_err(|_| CONFIG_UNSUPPORTED)?;

    lora.set_sync_word(profile.sync_word)
        .await
        .map_err(|_| CONFIG_RADIO_FAULT)?;

    Ok(Applied {
        modulation,
        tx,
        rx,
        tx_power_dbm: i32::from(profile.tx_power_dbm),
    })
}

/// The result code for a command whose bytes did not decode into a profile.
///
/// A caller that fails before it has a profile reports this; it is named here so the two
/// images do not each write a bare `1`.
pub const MALFORMED: u8 = CONFIG_MALFORMED;

/// The result code for a profile that applied cleanly.
pub const ACCEPTED: u8 = CONFIG_ACCEPTED;
