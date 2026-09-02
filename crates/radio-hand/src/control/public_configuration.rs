//! Strict portable configuration facts that a board adapter may apply.
//!
//! This is deliberately limited to the resident Retinue executive's radio and
//! management policy. Credentials, node identity, board channel selection,
//! and protocol-personality state do not belong in this portable public body.

use selvage::{CONFIG_COMMAND_LEN, PhyProfile, decode_config_command, encode_config_command};

use crate::region::Region;

use super::ManagementCarrier;

/// Schema byte preceding every [`PublicConfigurationV1`] encoding.
pub const PUBLIC_CONFIGURATION_V1_VERSION: u8 = 1;
/// Exact bytes in a canonical [`PublicConfigurationV1`] encoding.
pub const PUBLIC_CONFIGURATION_V1_LEN: usize = 5 + CONFIG_COMMAND_LEN;

const KNOWN_CARRIER_BITS: u8 = (1 << (ManagementCarrier::Usb as u8))
    | (1 << (ManagementCarrier::Ble as u8))
    | (1 << (ManagementCarrier::Ip as u8))
    | (1 << (ManagementCarrier::Reticulum as u8));
const RELAY_ANNOUNCES: u8 = 1;
const RELAY_PACKETS: u8 = 1 << 1;
const KNOWN_TRANSPORT_BITS: u8 = RELAY_ANNOUNCES | RELAY_PACKETS;

/// Validation failure for the portable public configuration encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicConfigurationError {
    Length,
    UnsupportedVersion(u8),
    InvalidRegion,
    EmptyManagementCarriers,
    UnknownManagementCarrierBits(u8),
    ReservedTransportBits(u8),
    NonCanonicalMaxHops,
    InvalidPhy,
    NonCanonicalPhy,
    PhyOutsideRegion,
}

/// The enabled management transports as a strict mask over [`ManagementCarrier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagementCarrierSet(u8);

impl ManagementCarrierSet {
    /// Constructs a non-empty set from its stable carrier bit mask.
    pub const fn from_mask(mask: u8) -> Result<Self, PublicConfigurationError> {
        if mask == 0 {
            return Err(PublicConfigurationError::EmptyManagementCarriers);
        }
        if mask & !KNOWN_CARRIER_BITS != 0 {
            return Err(PublicConfigurationError::UnknownManagementCarrierBits(mask));
        }
        Ok(Self(mask))
    }

    /// Returns the canonical stable carrier bit mask.
    pub const fn mask(self) -> u8 {
        self.0
    }

    /// Whether a carrier is enabled by this configuration.
    pub const fn contains(self, carrier: ManagementCarrier) -> bool {
        self.0 & (1 << (carrier as u8)) != 0
    }
}

/// Reticulum forwarding policy applied by the resident executive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReticulumTransportPolicy {
    pub relay_announces: bool,
    pub relay_packets: bool,
    pub max_hops: u8,
}

impl ReticulumTransportPolicy {
    /// Constructs the canonical forwarding policy.
    pub const fn new(
        relay_announces: bool,
        relay_packets: bool,
        max_hops: u8,
    ) -> Result<Self, PublicConfigurationError> {
        if !relay_announces && !relay_packets {
            if max_hops != 0 {
                return Err(PublicConfigurationError::NonCanonicalMaxHops);
            }
        } else if max_hops == 0 || max_hops > 128 {
            return Err(PublicConfigurationError::NonCanonicalMaxHops);
        }
        Ok(Self {
            relay_announces,
            relay_packets,
            max_hops,
        })
    }

    const fn flags(self) -> u8 {
        (self.relay_announces as u8) | ((self.relay_packets as u8) << 1)
    }
}

/// Board-independent, non-secret settings that a configuration applier may use.
///
/// Construction and decoding validate both the radio command and its selected
/// region. The exact byte representation is intentionally independent of board
/// flash geometry and contains neither credentials nor identity material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicConfigurationV1 {
    region: Region,
    reticulum_phy: PhyProfile,
    reticulum_transport: ReticulumTransportPolicy,
    enabled_management_carriers: ManagementCarrierSet,
}

impl PublicConfigurationV1 {
    /// Constructs a portable configuration after validating all cross-field facts.
    pub fn new(
        region: Region,
        reticulum_phy: PhyProfile,
        reticulum_transport: ReticulumTransportPolicy,
        enabled_management_carriers: ManagementCarrierSet,
    ) -> Result<Self, PublicConfigurationError> {
        validate(
            region,
            reticulum_phy,
            reticulum_transport,
            enabled_management_carriers,
        )?;
        Ok(Self {
            region,
            reticulum_phy,
            reticulum_transport,
            enabled_management_carriers,
        })
    }

    /// Revalidates this portable configuration without changing its canonical form.
    ///
    /// This is useful at trust boundaries that accept an already-decoded public
    /// configuration, such as initial owner commissioning.
    pub fn validate(self) -> Result<(), PublicConfigurationError> {
        validate(
            self.region,
            self.reticulum_phy,
            self.reticulum_transport,
            self.enabled_management_carriers,
        )
    }

    /// Selected regulatory region. It is always a transmitting region.
    pub const fn region(self) -> Region {
        self.region
    }

    /// The raw tx-power request retained in durable storage for Reticulum.
    ///
    /// This is not necessarily the power a board may transmit. A board applier
    /// must use [`Self::effective_reticulum_phy`] or the executive's
    /// `Executive::apply_profile` before reaching a radio
    /// service.
    pub const fn requested_reticulum_phy(self) -> PhyProfile {
        self.reticulum_phy
    }

    /// The Reticulum PHY with requested power clamped to this region and hardware.
    ///
    /// The persisted request remains unchanged, so a later board with a
    /// different hardware ceiling can derive its own lawful effective profile.
    pub fn effective_reticulum_phy(self, hardware_max_dbm: i8) -> PhyProfile {
        let mut profile = self.requested_reticulum_phy();
        profile.tx_power_dbm = self
            .region
            .profile()
            .expect("PublicConfigurationV1 construction validates region")
            .clamp_power(profile.tx_power_dbm, hardware_max_dbm);
        profile
    }

    /// Forwarding policy for the resident Reticulum adapter.
    pub const fn reticulum_transport(self) -> ReticulumTransportPolicy {
        self.reticulum_transport
    }

    /// Enabled management carriers.
    pub const fn enabled_management_carriers(self) -> ManagementCarrierSet {
        self.enabled_management_carriers
    }

    /// Encodes the one and only canonical 21-byte representation.
    pub fn encode(self) -> [u8; PUBLIC_CONFIGURATION_V1_LEN] {
        let mut bytes = [0; PUBLIC_CONFIGURATION_V1_LEN];
        bytes[0] = PUBLIC_CONFIGURATION_V1_VERSION;
        bytes[1] = self.region.as_byte();
        bytes[2] = self.enabled_management_carriers.mask();
        bytes[3] = self.reticulum_transport.flags();
        bytes[4] = self.reticulum_transport.max_hops;
        let phy = encode_config_command(self.reticulum_phy)
            .expect("PublicConfigurationV1 construction validates PHY");
        bytes[5..].copy_from_slice(&phy);
        bytes
    }

    /// Decodes exactly one canonical portable public configuration.
    pub fn decode(bytes: &[u8]) -> Result<Self, PublicConfigurationError> {
        if bytes.len() != PUBLIC_CONFIGURATION_V1_LEN {
            return Err(PublicConfigurationError::Length);
        }
        if bytes[0] != PUBLIC_CONFIGURATION_V1_VERSION {
            return Err(PublicConfigurationError::UnsupportedVersion(bytes[0]));
        }
        let region = Region::from_byte(bytes[1]).ok_or(PublicConfigurationError::InvalidRegion)?;
        let carriers = ManagementCarrierSet::from_mask(bytes[2])?;
        if bytes[3] & !KNOWN_TRANSPORT_BITS != 0 {
            return Err(PublicConfigurationError::ReservedTransportBits(bytes[3]));
        }
        let transport = ReticulumTransportPolicy::new(
            bytes[3] & RELAY_ANNOUNCES != 0,
            bytes[3] & RELAY_PACKETS != 0,
            bytes[4],
        )?;
        let phy =
            decode_config_command(&bytes[5..]).map_err(|_| PublicConfigurationError::InvalidPhy)?;
        let canonical =
            encode_config_command(phy).map_err(|_| PublicConfigurationError::InvalidPhy)?;
        if bytes[5..] != canonical {
            return Err(PublicConfigurationError::NonCanonicalPhy);
        }
        Self::new(region, phy, transport, carriers)
    }
}

fn validate(
    region: Region,
    phy: PhyProfile,
    transport: ReticulumTransportPolicy,
    carriers: ManagementCarrierSet,
) -> Result<(), PublicConfigurationError> {
    let profile = region
        .profile()
        .ok_or(PublicConfigurationError::InvalidRegion)?;
    encode_config_command(phy).map_err(|_| PublicConfigurationError::InvalidPhy)?;
    if !profile.allows_frequency(phy.frequency_hz) {
        return Err(PublicConfigurationError::PhyOutsideRegion);
    }
    ReticulumTransportPolicy::new(
        transport.relay_announces,
        transport.relay_packets,
        transport.max_hops,
    )?;
    ManagementCarrierSet::from_mask(carriers.mask())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reticulum_phy(frequency_hz: u32) -> PhyProfile {
        PhyProfile {
            frequency_hz,
            bandwidth_hz: 250_000,
            spreading_factor: 11,
            coding_rate_denominator: 5,
            preamble_symbols: 16,
            sync_word: 0x2d,
            explicit_header: true,
            crc: true,
            invert_iq: false,
            tx_power_dbm: 17,
        }
    }

    fn configuration() -> PublicConfigurationV1 {
        PublicConfigurationV1::new(
            Region::Us915,
            reticulum_phy(906_875_000),
            ReticulumTransportPolicy::new(true, true, 8).unwrap(),
            ManagementCarrierSet::from_mask(0b1001).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn golden_bytes_are_fixed_and_round_trip() {
        let bytes = configuration().encode();
        assert_eq!(
            bytes,
            [
                1, 1, 9, 3, 8, 2, 120, 208, 13, 54, 144, 208, 3, 0, 11, 5, 16, 0, 45, 3, 17,
            ]
        );
        assert_eq!(PublicConfigurationV1::decode(&bytes), Ok(configuration()));
    }

    #[test]
    fn strict_decoder_rejects_length_version_reserved_and_noncanonical_fields() {
        let bytes = configuration().encode();
        assert_eq!(
            PublicConfigurationV1::decode(&bytes[..20]),
            Err(PublicConfigurationError::Length)
        );
        let mut trailing = [0; PUBLIC_CONFIGURATION_V1_LEN + 1];
        trailing[..PUBLIC_CONFIGURATION_V1_LEN].copy_from_slice(&bytes);
        assert_eq!(
            PublicConfigurationV1::decode(&trailing),
            Err(PublicConfigurationError::Length)
        );
        let mut changed = bytes;
        changed[0] = 2;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::UnsupportedVersion(2))
        );
        changed = bytes;
        changed[1] = Region::Unset.as_byte();
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::InvalidRegion)
        );
        changed = bytes;
        changed[1] = 0xff;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::InvalidRegion)
        );
        changed = bytes;
        changed[2] = 0;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::EmptyManagementCarriers)
        );
        changed = bytes;
        changed[2] = 0x80;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::UnknownManagementCarrierBits(0x80))
        );
        changed = bytes;
        changed[3] = 0x80;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::ReservedTransportBits(0x80))
        );
        changed = bytes;
        changed[4] = 0;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::NonCanonicalMaxHops)
        );
        changed = bytes;
        changed[3] = 0;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::NonCanonicalMaxHops)
        );
        changed = bytes;
        changed[14] = 4;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::InvalidPhy)
        );
        changed = bytes;
        changed[19] |= 0x80;
        assert_eq!(
            PublicConfigurationV1::decode(&changed),
            Err(PublicConfigurationError::NonCanonicalPhy)
        );
    }

    #[test]
    fn region_and_phy_are_bound_together() {
        assert_eq!(
            PublicConfigurationV1::new(
                Region::Unset,
                reticulum_phy(906_875_000),
                ReticulumTransportPolicy::new(false, false, 0).unwrap(),
                ManagementCarrierSet::from_mask(1).unwrap(),
            ),
            Err(PublicConfigurationError::InvalidRegion)
        );
        assert_eq!(
            PublicConfigurationV1::new(
                Region::Eu868,
                reticulum_phy(906_875_000),
                ReticulumTransportPolicy::new(false, false, 0).unwrap(),
                ManagementCarrierSet::from_mask(1).unwrap(),
            ),
            Err(PublicConfigurationError::PhyOutsideRegion)
        );
    }

    #[test]
    fn requested_power_round_trips_while_effective_power_is_clamped() {
        let mut requested = reticulum_phy(921_875_000);
        requested.tx_power_dbm = 22;
        let configuration = PublicConfigurationV1::new(
            Region::Jp920,
            requested,
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap();
        let decoded = PublicConfigurationV1::decode(&configuration.encode()).unwrap();
        assert_eq!(decoded.requested_reticulum_phy().tx_power_dbm, 22);
        assert_eq!(decoded.effective_reticulum_phy(22).tx_power_dbm, 13);
        assert_eq!(decoded.effective_reticulum_phy(8).tx_power_dbm, 8);
    }
}
