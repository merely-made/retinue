//! What a board persists: its identity, and which channel it boots into.
//!
//! This is the *body* of a [`store`](crate::store) record. The store owns durability (two
//! slots, a sequence, a checksum, torn-write recovery); this owns what the surviving bytes
//! mean.
//!
//! # Why the body is extensible rather than versioned
//!
//! The record header already carries a version, and bumping it would be the obvious way to
//! add a field. It is the wrong way here: a board flashed with the new firmware would find
//! its existing record at the old version, refuse it, and mint a fresh identity. The device
//! would silently become a different device, losing the address every peer knows it by.
//!
//! So the body grows instead. A 64-byte body is an identity and nothing else, which is
//! exactly what the first firmware wrote, and it decodes today as that identity with
//! default settings. Longer bodies carry fields appended after it. Old records keep
//! working, and a board that downgrades keeps its identity because the first 64 bytes never
//! move.

/// Bytes of a device identity: an ed25519 signing key followed by an x25519 encryption key.
pub const IDENTITY_LEN: usize = 64;

/// Bytes this module writes. Room for the identity and the fields after it.
pub const ENCODED_LEN: usize = IDENTITY_LEN + 4;

/// Which personality the board boots into.
///
/// One radio means one mesh at a time (one PHY configuration, one sync word), so this is a
/// choice rather than a mixture. See the plan's structural decision 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Channel {
    /// The host-driven direct-PHY modem. The default, and the recovery personality: it is
    /// the one that always works and needs no on-board protocol state.
    #[default]
    Modem = 0,
    /// The native Retinue node. The board holds the identity and answers for itself.
    Node = 1,
    /// The board as an RNode: the same host-driven radio the modem is, speaking the protocol
    /// stock Reticulum software already knows how to drive.
    Rnode = 2,
}

impl Channel {
    /// The channel a stored byte names, or `None` if this build does not know it.
    ///
    /// An unknown channel is refused rather than guessed: a board that downgraded past a
    /// channel it once ran must fall back to something it can actually serve.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Channel::Modem),
            1 => Some(Channel::Node),
            2 => Some(Channel::Rnode),
            _ => None,
        }
    }

    pub fn as_byte(self) -> u8 {
        self as u8
    }
}

/// The persisted board settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The device identity. Never leaves the board.
    pub identity: [u8; IDENTITY_LEN],
    /// The channel to boot into.
    pub channel: Channel,
    /// The regulatory region this board operates under. `Unset` means no transmit.
    pub region: crate::region::Region,
}

/// Why a stored body could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    /// Fewer bytes than an identity. Not a record this firmware ever wrote.
    Truncated,
}

impl Settings {
    /// Settings for a board that has just generated an identity.
    pub fn new(identity: [u8; IDENTITY_LEN]) -> Self {
        Self {
            identity,
            channel: Channel::default(),
            region: crate::region::Region::default(),
        }
    }

    /// Read settings from a stored record body.
    ///
    /// A 64-byte body is the original identity-only record and decodes with default
    /// settings. A field this build does not understand, including a channel byte it does
    /// not know, falls back to its default rather than failing the whole record: losing the
    /// identity over an unreadable preference would be a far worse trade.
    pub fn decode(body: &[u8]) -> Result<Self, SettingsError> {
        if body.len() < IDENTITY_LEN {
            return Err(SettingsError::Truncated);
        }
        let mut identity = [0_u8; IDENTITY_LEN];
        identity.copy_from_slice(&body[..IDENTITY_LEN]);

        let channel = body
            .get(IDENTITY_LEN)
            .copied()
            .and_then(Channel::from_byte)
            .unwrap_or_default();

        // The region byte was reserved-as-zero before regions existed, and Region::Unset
        // is zero, so every record written before this field upgrades into "no region" —
        // never into someone else's rules. An unknown byte falls back the same way.
        let region = body
            .get(IDENTITY_LEN + 1)
            .copied()
            .and_then(crate::region::Region::from_byte)
            .unwrap_or_default();

        Ok(Self {
            identity,
            channel,
            region,
        })
    }

    /// Write settings into a record body, returning the bytes used.
    pub fn encode(&self, out: &mut [u8; ENCODED_LEN]) -> usize {
        out[..IDENTITY_LEN].copy_from_slice(&self.identity);
        out[IDENTITY_LEN] = self.channel.as_byte();
        out[IDENTITY_LEN + 1] = self.region.as_byte();
        // Reserved, written as zero so a later field has a defined starting value rather
        // than whatever the erase left.
        out[IDENTITY_LEN + 2..].fill(0);
        ENCODED_LEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> [u8; IDENTITY_LEN] {
        let mut id = [0_u8; IDENTITY_LEN];
        for (i, byte) in id.iter_mut().enumerate() {
            *byte = i as u8;
        }
        id
    }

    #[test]
    fn round_trips() {
        let settings = Settings {
            identity: identity(),
            channel: Channel::Node,
            region: crate::region::Region::Us915,
        };
        let mut body = [0_u8; ENCODED_LEN];
        let len = settings.encode(&mut body);
        assert_eq!(Settings::decode(&body[..len]).unwrap(), settings);
    }

    /// The record the first firmware wrote, 64 bytes of identity and nothing else, still
    /// reads. This is the property that keeps a board's identity across the upgrade that
    /// introduced settings, which is the whole reason the body grows instead of versioning.
    #[test]
    fn a_legacy_identity_only_body_keeps_its_identity() {
        let legacy = identity();
        let read = Settings::decode(&legacy).expect("the original record must still read");
        assert_eq!(read.identity, legacy, "the identity survives verbatim");
        assert_eq!(read.channel, Channel::Modem, "and defaults to the modem");
        assert_eq!(
            read.region,
            crate::region::Region::Unset,
            "and to no region: an old record never inherits someone's rules"
        );
    }

    /// A channel byte this build does not know falls back rather than failing the record.
    #[test]
    fn an_unknown_channel_falls_back_without_losing_the_identity() {
        let mut body = [0_u8; ENCODED_LEN];
        body[..IDENTITY_LEN].copy_from_slice(&identity());
        body[IDENTITY_LEN] = 0xEE;

        let read = Settings::decode(&body).expect("an odd preference is not a bad record");
        assert_eq!(read.identity, identity(), "the identity is what matters");
        assert_eq!(read.channel, Channel::Modem, "and the board still boots");
        assert_eq!(read.region, crate::region::Region::Unset);
    }

    /// Trailing bytes a later firmware appended are ignored, not misread.
    #[test]
    fn a_longer_body_from_a_later_firmware_is_read_as_far_as_it_is_understood() {
        let mut body = [0_u8; ENCODED_LEN + 32];
        body[..IDENTITY_LEN].copy_from_slice(&identity());
        body[IDENTITY_LEN] = Channel::Node.as_byte();
        body[ENCODED_LEN..].fill(0xAB);

        let read = Settings::decode(&body).unwrap();
        assert_eq!(read.identity, identity());
        assert_eq!(read.channel, Channel::Node);
    }

    #[test]
    fn a_body_shorter_than_an_identity_is_refused() {
        assert_eq!(
            Settings::decode(&[0_u8; IDENTITY_LEN - 1]),
            Err(SettingsError::Truncated)
        );
    }

    /// Every channel this build knows survives a round trip, so adding one cannot silently
    /// break the ones already shipped.
    #[test]
    fn every_known_channel_round_trips() {
        for channel in [Channel::Modem, Channel::Node, Channel::Rnode] {
            let settings = Settings {
                identity: identity(),
                channel,
                region: crate::region::Region::Unset,
            };
            let mut body = [0_u8; ENCODED_LEN];
            let len = settings.encode(&mut body);
            assert_eq!(Settings::decode(&body[..len]).unwrap().channel, channel);
            assert_eq!(Channel::from_byte(channel.as_byte()), Some(channel));
        }
    }
}
