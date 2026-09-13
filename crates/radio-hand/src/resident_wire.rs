//! Fixed-size USB commissioning command for the optional resident protocols.
//!
//! This is deliberately a one-shot configuration record, not a general packet
//! tunnel.  The board accepts it only at a direct-PHY command boundary, copies
//! no caller-controlled allocation, and builds fresh resident state from it.

use selvage::PhyProfile;

/// Host-to-board marker reserved for resident-protocol setup.
pub const CMD_RESIDENT_SETUP: u8 = 0x07;
pub const RESIDENT_SETUP_VERSION: u8 = 1;
pub const RESIDENT_PROFILE_COUNT: usize = 3;
pub const RESIDENT_NAME_HASH_LEN: usize = 10;
pub const RESIDENT_SETUP_LEN: usize = 185;

const OFFSET_VERSION: usize = 1;
const OFFSET_SOURCE: usize = 2;
const OFFSET_CHANNEL: usize = 6;
const OFFSET_KEY_LEN: usize = 7;
const OFFSET_KEY: usize = 8;
const OFFSET_TUCKET_SEED: usize = 40;
const OFFSET_NAME_HASH: usize = 72;
const OFFSET_PROFILES: usize = 82;
const PROFILE_LEN: usize = 16;
const OFFSET_HOME: usize = 130;
const OFFSET_PIN: usize = 131;
const OFFSET_COVERAGE: usize = 132;
const OFFSET_MAX_EXCURSION: usize = 133;
const OFFSET_RETURN_BUDGET: usize = 141;
const OFFSET_MAX_DEFER: usize = 149;
const OFFSET_TRANSITION_TIMEOUT: usize = 157;
const OFFSET_TX_BUDGET: usize = 165;
const OFFSET_FRAME_TTL: usize = 173;
const OFFSET_PACKET_LEASE: usize = 181;

/// A validated Sennet key supplied by the locally attached USB host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SennetKey {
    Aes128([u8; 16]),
    Aes256([u8; 32]),
}

/// The complete bounded resident setup requested by a host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResidentSetup {
    pub sennet_source: u32,
    pub sennet_channel: u8,
    pub sennet_key: SennetKey,
    /// This seed is distinct from the durable Retinue identity held by the board.
    pub tucket_identity_seed: [u8; 32],
    pub retinue_name_hash: [u8; RESIDENT_NAME_HASH_LEN],
    /// Profiles are Retinue, Sennet, then Tucket, in the runtime's fixed ID order.
    pub profiles: [PhyProfile; RESIDENT_PROFILE_COUNT],
    pub home: u8,
    pub pin: Option<u8>,
    pub require_coverage: bool,
    pub max_excursion_ms: u64,
    pub return_budget_ms: u64,
    pub max_defer_ms: u64,
    pub transition_timeout_ms: u64,
    pub tx_budget_ms: u64,
    pub frame_ttl_ms: u64,
    /// Number of packet IDs which storage must durably reserve before construction.
    pub packet_lease_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidentWireError {
    Length { actual: usize },
    Marker,
    Version(u8),
    KeyLength(u8),
    Profile { index: usize },
    Home(u8),
    Pin(u8),
    PinDiffersFromHome { pin: u8, home: u8 },
    Coverage(u8),
    MissingSecret,
    ZeroBudget,
    ZeroPacketLease,
}

impl ResidentSetup {
    /// Decode and validate the fixed command before any protocol allocation.
    pub fn decode(frame: &[u8]) -> Result<Self, ResidentWireError> {
        if frame.len() != RESIDENT_SETUP_LEN {
            return Err(ResidentWireError::Length {
                actual: frame.len(),
            });
        }
        if frame[0] != CMD_RESIDENT_SETUP {
            return Err(ResidentWireError::Marker);
        }
        if frame[OFFSET_VERSION] != RESIDENT_SETUP_VERSION {
            return Err(ResidentWireError::Version(frame[OFFSET_VERSION]));
        }
        let mut key = [0_u8; 32];
        key.copy_from_slice(&frame[OFFSET_KEY..OFFSET_KEY + 32]);
        let sennet_key = match frame[OFFSET_KEY_LEN] {
            16 => {
                let mut short = [0_u8; 16];
                short.copy_from_slice(&key[..16]);
                SennetKey::Aes128(short)
            }
            32 => SennetKey::Aes256(key),
            other => return Err(ResidentWireError::KeyLength(other)),
        };
        let mut seed = [0_u8; 32];
        seed.copy_from_slice(&frame[OFFSET_TUCKET_SEED..OFFSET_TUCKET_SEED + 32]);
        let mut name_hash = [0_u8; RESIDENT_NAME_HASH_LEN];
        name_hash
            .copy_from_slice(&frame[OFFSET_NAME_HASH..OFFSET_NAME_HASH + RESIDENT_NAME_HASH_LEN]);
        let mut profiles = [PhyProfile::meshtastic_long_fast(1); RESIDENT_PROFILE_COUNT];
        for (index, profile) in profiles.iter_mut().enumerate() {
            *profile =
                decode_profile(&frame[OFFSET_PROFILES + index * PROFILE_LEN..][..PROFILE_LEN])
                    .map_err(|_| ResidentWireError::Profile { index })?;
        }
        let pin = match frame[OFFSET_PIN] {
            0xff => None,
            value if value < RESIDENT_PROFILE_COUNT as u8 => Some(value),
            value => return Err(ResidentWireError::Pin(value)),
        };
        let require_coverage = match frame[OFFSET_COVERAGE] {
            0 => false,
            1 => true,
            value => return Err(ResidentWireError::Coverage(value)),
        };
        let setup = Self {
            sennet_source: le_u32(frame, OFFSET_SOURCE),
            sennet_channel: frame[OFFSET_CHANNEL],
            sennet_key,
            tucket_identity_seed: seed,
            retinue_name_hash: name_hash,
            profiles,
            home: frame[OFFSET_HOME],
            pin,
            require_coverage,
            max_excursion_ms: le_u64(frame, OFFSET_MAX_EXCURSION),
            return_budget_ms: le_u64(frame, OFFSET_RETURN_BUDGET),
            max_defer_ms: le_u64(frame, OFFSET_MAX_DEFER),
            transition_timeout_ms: le_u64(frame, OFFSET_TRANSITION_TIMEOUT),
            tx_budget_ms: le_u64(frame, OFFSET_TX_BUDGET),
            frame_ttl_ms: le_u64(frame, OFFSET_FRAME_TTL),
            packet_lease_count: le_u32(frame, OFFSET_PACKET_LEASE),
        };
        if setup.home >= RESIDENT_PROFILE_COUNT as u8 {
            return Err(ResidentWireError::Home(setup.home));
        }
        if let Some(pin) = setup.pin
            && pin != setup.home
        {
            return Err(ResidentWireError::PinDiffersFromHome {
                pin,
                home: setup.home,
            });
        }
        if setup.max_excursion_ms == 0
            || setup.return_budget_ms == 0
            || setup.max_defer_ms == 0
            || setup.transition_timeout_ms == 0
            || setup.tx_budget_ms == 0
            || setup.frame_ttl_ms == 0
        {
            return Err(ResidentWireError::ZeroBudget);
        }
        if setup.packet_lease_count == 0 {
            return Err(ResidentWireError::ZeroPacketLease);
        }
        if setup.tucket_identity_seed.iter().all(|byte| *byte == 0)
            || setup.retinue_name_hash.iter().all(|byte| *byte == 0)
            || match setup.sennet_key {
                SennetKey::Aes128(key) => key.iter().all(|byte| *byte == 0),
                SennetKey::Aes256(key) => key.iter().all(|byte| *byte == 0),
            }
        {
            return Err(ResidentWireError::MissingSecret);
        }
        Ok(setup)
    }

    /// Encode a configuration for a local USB commissioning client or host test.
    pub fn encode(self) -> [u8; RESIDENT_SETUP_LEN] {
        let mut frame = [0_u8; RESIDENT_SETUP_LEN];
        frame[0] = CMD_RESIDENT_SETUP;
        frame[OFFSET_VERSION] = RESIDENT_SETUP_VERSION;
        frame[OFFSET_SOURCE..OFFSET_SOURCE + 4].copy_from_slice(&self.sennet_source.to_le_bytes());
        frame[OFFSET_CHANNEL] = self.sennet_channel;
        match self.sennet_key {
            SennetKey::Aes128(key) => {
                frame[OFFSET_KEY_LEN] = 16;
                frame[OFFSET_KEY..OFFSET_KEY + 16].copy_from_slice(&key);
            }
            SennetKey::Aes256(key) => {
                frame[OFFSET_KEY_LEN] = 32;
                frame[OFFSET_KEY..OFFSET_KEY + 32].copy_from_slice(&key);
            }
        }
        frame[OFFSET_TUCKET_SEED..OFFSET_TUCKET_SEED + 32]
            .copy_from_slice(&self.tucket_identity_seed);
        frame[OFFSET_NAME_HASH..OFFSET_NAME_HASH + RESIDENT_NAME_HASH_LEN]
            .copy_from_slice(&self.retinue_name_hash);
        for (index, profile) in self.profiles.iter().copied().enumerate() {
            encode_profile(
                profile,
                &mut frame[OFFSET_PROFILES + index * PROFILE_LEN..][..PROFILE_LEN],
            );
        }
        frame[OFFSET_HOME] = self.home;
        frame[OFFSET_PIN] = self.pin.unwrap_or(0xff);
        frame[OFFSET_COVERAGE] = u8::from(self.require_coverage);
        frame[OFFSET_MAX_EXCURSION..OFFSET_MAX_EXCURSION + 8]
            .copy_from_slice(&self.max_excursion_ms.to_le_bytes());
        frame[OFFSET_RETURN_BUDGET..OFFSET_RETURN_BUDGET + 8]
            .copy_from_slice(&self.return_budget_ms.to_le_bytes());
        frame[OFFSET_MAX_DEFER..OFFSET_MAX_DEFER + 8]
            .copy_from_slice(&self.max_defer_ms.to_le_bytes());
        frame[OFFSET_TRANSITION_TIMEOUT..OFFSET_TRANSITION_TIMEOUT + 8]
            .copy_from_slice(&self.transition_timeout_ms.to_le_bytes());
        frame[OFFSET_TX_BUDGET..OFFSET_TX_BUDGET + 8]
            .copy_from_slice(&self.tx_budget_ms.to_le_bytes());
        frame[OFFSET_FRAME_TTL..OFFSET_FRAME_TTL + 8]
            .copy_from_slice(&self.frame_ttl_ms.to_le_bytes());
        frame[OFFSET_PACKET_LEASE..].copy_from_slice(&self.packet_lease_count.to_le_bytes());
        frame
    }
}

/// Result from incrementally demultiplexing USB bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidentSetupByte {
    Ordinary(u8),
    Pending,
    Complete(ResidentSetup),
    Rejected(ResidentWireError),
}

/// Fixed storage for a fragmented setup command. It never allocates.
pub struct ResidentSetupStream {
    bytes: [u8; RESIDENT_SETUP_LEN],
    len: usize,
}

impl ResidentSetupStream {
    pub const fn new() -> Self {
        Self {
            bytes: [0; RESIDENT_SETUP_LEN],
            len: 0,
        }
    }
    pub const fn pending(&self) -> bool {
        self.len != 0
    }
    pub const fn at_boundary(&self) -> bool {
        self.len == 0
    }
    pub fn push(&mut self, at_boundary: bool, byte: u8) -> ResidentSetupByte {
        if self.len == 0 {
            if !at_boundary || byte != CMD_RESIDENT_SETUP {
                return ResidentSetupByte::Ordinary(byte);
            }
            self.bytes[0] = byte;
            self.len = 1;
            return ResidentSetupByte::Pending;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        if self.len != RESIDENT_SETUP_LEN {
            return ResidentSetupByte::Pending;
        }
        self.len = 0;
        match ResidentSetup::decode(&self.bytes) {
            Ok(setup) => ResidentSetupByte::Complete(setup),
            Err(error) => ResidentSetupByte::Rejected(error),
        }
    }
}
impl Default for ResidentSetupStream {
    fn default() -> Self {
        Self::new()
    }
}

fn le_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed resident setup"),
    )
}
fn le_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("fixed resident setup"),
    )
}

fn decode_profile(bytes: &[u8]) -> Result<PhyProfile, ()> {
    let flags = bytes[13];
    if flags & !0b111 != 0 || bytes[15] != 0 {
        return Err(());
    }
    PhyProfile {
        frequency_hz: le_u32(bytes, 0),
        bandwidth_hz: le_u32(bytes, 4),
        spreading_factor: bytes[8],
        coding_rate_denominator: bytes[9],
        preamble_symbols: u16::from_le_bytes(bytes[10..12].try_into().expect("fixed profile")),
        sync_word: bytes[12],
        explicit_header: flags & 1 != 0,
        crc: flags & 2 != 0,
        invert_iq: flags & 4 != 0,
        tx_power_dbm: bytes[14] as i8,
    }
    .validate()
    .map_err(|_| ())
}
fn encode_profile(profile: PhyProfile, out: &mut [u8]) {
    out[..4].copy_from_slice(&profile.frequency_hz.to_le_bytes());
    out[4..8].copy_from_slice(&profile.bandwidth_hz.to_le_bytes());
    out[8] = profile.spreading_factor;
    out[9] = profile.coding_rate_denominator;
    out[10..12].copy_from_slice(&profile.preamble_symbols.to_le_bytes());
    out[12] = profile.sync_word;
    out[13] = u8::from(profile.explicit_header)
        | (u8::from(profile.crc) << 1)
        | (u8::from(profile.invert_iq) << 2);
    out[14] = profile.tx_power_dbm as u8;
    out[15] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> ResidentSetup {
        ResidentSetup {
            sennet_source: 0x1020_3040,
            sennet_channel: 8,
            sennet_key: SennetKey::Aes256([3; 32]),
            tucket_identity_seed: [4; 32],
            retinue_name_hash: [5; 10],
            profiles: [PhyProfile::meshtastic_long_fast(906_875_000); 3],
            home: 0,
            pin: Some(0),
            require_coverage: false,
            max_excursion_ms: 60_000,
            return_budget_ms: 2_000,
            max_defer_ms: 1,
            transition_timeout_ms: 1_500,
            tx_budget_ms: 900,
            frame_ttl_ms: 5_000,
            packet_lease_count: 128,
        }
    }
    #[test]
    fn round_trip_is_fixed_and_lossless() {
        let value = setup();
        let wire = value.encode();
        assert_eq!(wire.len(), RESIDENT_SETUP_LEN);
        assert_eq!(ResidentSetup::decode(&wire), Ok(value));
    }
    #[test]
    fn malformed_profile_and_zero_lease_are_refused() {
        let mut wire = setup().encode();
        wire[OFFSET_PROFILES + 13] = 0x80;
        assert_eq!(
            ResidentSetup::decode(&wire),
            Err(ResidentWireError::Profile { index: 0 })
        );
        let mut wire = setup().encode();
        wire[OFFSET_PACKET_LEASE..].fill(0);
        assert_eq!(
            ResidentSetup::decode(&wire),
            Err(ResidentWireError::ZeroPacketLease)
        );
        let mut wire = setup().encode();
        wire[OFFSET_PROFILES + 15] = 1;
        assert_eq!(
            ResidentSetup::decode(&wire),
            Err(ResidentWireError::Profile { index: 0 })
        );
    }
    #[test]
    fn fragmented_stream_never_eats_an_ordinary_marker_in_payload() {
        let wire = setup().encode();
        let mut stream = ResidentSetupStream::new();
        assert_eq!(
            stream.push(false, CMD_RESIDENT_SETUP),
            ResidentSetupByte::Ordinary(CMD_RESIDENT_SETUP)
        );
        for (at, byte) in wire.iter().copied().enumerate() {
            let event = stream.push(at == 0, byte);
            if at + 1 == wire.len() {
                assert_eq!(event, ResidentSetupByte::Complete(setup()));
            } else {
                assert_eq!(event, ResidentSetupByte::Pending);
            }
        }
    }
}
