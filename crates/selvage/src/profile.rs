//! HAL-independent radio profiles and their config and excursion commands.

use crate::{
    CMD_CONFIG, CMD_EXCURSION, CONFIG_COMMAND_LEN, EXCURSION_COMMAND_LEN, MESHCORE_SYNC_WORD,
    MESHTASTIC_SYNC_WORD,
};

/// Radio parameters that are independent of a particular HAL or driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyProfile {
    pub frequency_hz: u32,
    pub bandwidth_hz: u32,
    pub spreading_factor: u8,
    pub coding_rate_denominator: u8,
    pub preamble_symbols: u16,
    pub sync_word: u8,
    pub explicit_header: bool,
    pub crc: bool,
    pub invert_iq: bool,
    pub tx_power_dbm: i8,
}

impl PhyProfile {
    /// Meshtastic LongFast modulation with a caller-selected regional frequency.
    pub const fn meshtastic_long_fast(frequency_hz: u32) -> Self {
        Self {
            frequency_hz,
            bandwidth_hz: 250_000,
            spreading_factor: 11,
            coding_rate_denominator: 5,
            preamble_symbols: 16,
            sync_word: MESHTASTIC_SYNC_WORD,
            explicit_header: true,
            crc: true,
            invert_iq: false,
            tx_power_dbm: 17,
        }
    }

    /// MeshCore modulation with caller-selected companion radio parameters.
    ///
    /// MeshCore lengthens the preamble to 32 symbols at SF5 through SF8 and
    /// otherwise uses 16. Frequency, bandwidth, spreading factor, and coding
    /// rate remain network settings rather than board defaults.
    pub const fn meshcore(
        frequency_hz: u32,
        bandwidth_hz: u32,
        spreading_factor: u8,
        coding_rate_denominator: u8,
    ) -> Self {
        Self {
            frequency_hz,
            bandwidth_hz,
            spreading_factor,
            coding_rate_denominator,
            preamble_symbols: if spreading_factor <= 8 { 32 } else { 16 },
            sync_word: MESHCORE_SYNC_WORD,
            explicit_header: true,
            crc: true,
            invert_iq: false,
            tx_power_dbm: 17,
        }
    }

    /// Validate the protocol-independent envelope accepted by Tulle firmware.
    pub const fn validate(self) -> Result<Self, ProfileError> {
        if self.frequency_hz == 0 {
            return Err(ProfileError::Frequency);
        }
        if self.bandwidth_hz == 0 {
            return Err(ProfileError::Bandwidth);
        }
        if self.spreading_factor < 5 || self.spreading_factor > 12 {
            return Err(ProfileError::SpreadingFactor);
        }
        if self.coding_rate_denominator < 5 || self.coding_rate_denominator > 8 {
            return Err(ProfileError::CodingRate);
        }
        if self.preamble_symbols == 0 {
            return Err(ProfileError::Preamble);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    Command,
    Length,
    Frequency,
    Bandwidth,
    SpreadingFactor,
    CodingRate,
    Preamble,
}

/// Encode a complete runtime radio-profile command.
pub const fn encode_config_command(
    profile: PhyProfile,
) -> Result<[u8; CONFIG_COMMAND_LEN], ProfileError> {
    let profile = match profile.validate() {
        Ok(profile) => profile,
        Err(error) => return Err(error),
    };
    let mut out = [0_u8; CONFIG_COMMAND_LEN];
    out[0] = CMD_CONFIG;
    let frequency = profile.frequency_hz.to_le_bytes();
    out[1] = frequency[0];
    out[2] = frequency[1];
    out[3] = frequency[2];
    out[4] = frequency[3];
    let bandwidth = profile.bandwidth_hz.to_le_bytes();
    out[5] = bandwidth[0];
    out[6] = bandwidth[1];
    out[7] = bandwidth[2];
    out[8] = bandwidth[3];
    out[9] = profile.spreading_factor;
    out[10] = profile.coding_rate_denominator;
    let preamble = profile.preamble_symbols.to_le_bytes();
    out[11] = preamble[0];
    out[12] = preamble[1];
    out[13] = profile.sync_word;
    out[14] = (profile.explicit_header as u8)
        | ((profile.crc as u8) << 1)
        | ((profile.invert_iq as u8) << 2);
    out[15] = profile.tx_power_dbm as u8;
    Ok(out)
}

/// Decode and validate a complete runtime radio-profile command.
pub fn decode_config_command(command: &[u8]) -> Result<PhyProfile, ProfileError> {
    if command.len() != CONFIG_COMMAND_LEN {
        return Err(ProfileError::Length);
    }
    if command[0] != CMD_CONFIG {
        return Err(ProfileError::Command);
    }
    PhyProfile {
        frequency_hz: u32::from_le_bytes([command[1], command[2], command[3], command[4]]),
        bandwidth_hz: u32::from_le_bytes([command[5], command[6], command[7], command[8]]),
        spreading_factor: command[9],
        coding_rate_denominator: command[10],
        preamble_symbols: u16::from_le_bytes([command[11], command[12]]),
        sync_word: command[13],
        explicit_header: command[14] & 1 != 0,
        crc: command[14] & 2 != 0,
        invert_iq: command[14] & 4 != 0,
        tx_power_dbm: command[15] as i8,
    }
    .validate()
}

/// Decode a bounded excursion request. The profile uses the normal canonical config body;
/// duration is caller-monotonic milliseconds and is deliberately not an RF packet format.
pub fn decode_excursion_command(command: &[u8]) -> Result<(PhyProfile, u64), ProfileError> {
    if command.len() != EXCURSION_COMMAND_LEN || command.first().copied() != Some(CMD_EXCURSION) {
        return Err(ProfileError::Length);
    }
    let mut profile_command = [0_u8; CONFIG_COMMAND_LEN];
    profile_command.copy_from_slice(&command[..CONFIG_COMMAND_LEN]);
    profile_command[0] = CMD_CONFIG;
    let profile = decode_config_command(&profile_command)?;
    let mut duration = [0_u8; 8];
    duration.copy_from_slice(&command[CONFIG_COMMAND_LEN..]);
    Ok((profile, u64::from_le_bytes(duration)))
}

/// Convert the canonical one-byte LoRa sync word to the SX126x register form.
pub const fn sx126x_sync_word(sync_word: u8) -> [u8; 2] {
    [(sync_word & 0xf0) | 0x04, ((sync_word & 0x0f) << 4) | 0x04]
}
