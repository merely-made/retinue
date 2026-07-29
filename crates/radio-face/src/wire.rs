use crate::status::{
    DetailPolicy, EventKind, EventSource, HostSnapshot, IfacState, NodeSummary, PeerPath,
    PeerSummary, Personality, Text, TextError, UiEvent,
};

pub const SNAPSHOT_VERSION: u8 = 1;
pub const MAX_SNAPSHOT_LEN: usize = 160;
pub const MAX_VALIDITY_SECS: u16 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    OutputTooSmall,
    Truncated,
    TooLong,
    UnsupportedVersion(u8),
    InvalidValidity(u16),
    InvalidEnum,
    InvalidFlags,
    PrivacyViolation,
    InvalidText(TextError),
    TrailingBytes,
}

impl From<TextError> for WireError {
    fn from(error: TextError) -> Self {
        Self::InvalidText(error)
    }
}

struct Writer<'a> {
    output: &'a mut [u8],
    offset: usize,
}

impl Writer<'_> {
    fn byte(&mut self, value: u8) -> Result<(), WireError> {
        self.bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), WireError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), WireError> {
        self.bytes(&value.to_le_bytes())
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), WireError> {
        let end = self
            .offset
            .checked_add(value.len())
            .ok_or(WireError::TooLong)?;
        let destination = self
            .output
            .get_mut(self.offset..end)
            .ok_or(WireError::OutputTooSmall)?;
        destination.copy_from_slice(value);
        self.offset = end;
        Ok(())
    }

    fn text<const N: usize>(&mut self, value: &Text<N>) -> Result<(), WireError> {
        self.byte(value.len() as u8)?;
        self.bytes(value.as_str().as_bytes())
    }
}

struct Reader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, WireError> {
        let value = *self.input.get(self.offset).ok_or(WireError::Truncated)?;
        self.offset += 1;
        Ok(value)
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], WireError> {
        let end = self.offset.checked_add(length).ok_or(WireError::TooLong)?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or(WireError::Truncated)?;
        self.offset = end;
        Ok(value)
    }

    fn text<const N: usize>(&mut self) -> Result<Text<N>, WireError> {
        let length = usize::from(self.byte()?);
        if length > N {
            return Err(WireError::TooLong);
        }
        Ok(Text::try_from_bytes(self.bytes(length)?)?)
    }
}

pub fn encode_snapshot(snapshot: &HostSnapshot, output: &mut [u8]) -> Result<usize, WireError> {
    if snapshot.valid_for_secs == 0 || snapshot.valid_for_secs > MAX_VALIDITY_SECS {
        return Err(WireError::InvalidValidity(snapshot.valid_for_secs));
    }
    if snapshot.detail == DetailPolicy::Minimal
        && (snapshot.node.is_some() || snapshot.peer_count() > 0 || snapshot.peer_overflow > 0)
    {
        return Err(WireError::PrivacyViolation);
    }

    let peer_count = snapshot.peer_count();
    let mut flags = 0_u8;
    if snapshot.ifac != IfacState::Unknown {
        flags |= 1 << 0;
    }
    if snapshot.ifac == IfacState::On {
        flags |= 1 << 1;
    }
    if snapshot.node.is_some() {
        flags |= 1 << 2;
    }
    if snapshot.event.is_some() {
        flags |= 1 << 3;
    }

    let mut writer = Writer { output, offset: 0 };
    writer.byte(SNAPSHOT_VERSION)?;
    writer.u16(snapshot.valid_for_secs)?;
    writer.byte(snapshot.personality as u8)?;
    writer.byte(snapshot.detail as u8)?;
    writer.byte(flags)?;
    writer.byte(snapshot.link_count)?;
    writer.byte(snapshot.admitted_links)?;
    writer.u16(snapshot.queue_depth)?;
    writer.byte(peer_count as u8)?;
    writer.byte(snapshot.peer_overflow)?;

    if let Some(node) = snapshot.node {
        writer.text(&node.name)?;
        writer.bytes(&node.address_tail)?;
        writer.bytes(&node.fingerprint)?;
        writer.text(&node.role)?;
        writer.u32(node.uptime_secs)?;
    }

    for peer in snapshot.peers.iter().flatten() {
        writer.text(&peer.name)?;
        writer.byte(peer.path as u8)?;
        writer.u32(peer.age_secs)?;
    }

    if let Some(event) = snapshot.event {
        writer.byte(event.source as u8)?;
        writer.byte(event.kind as u8)?;
        writer.text(&event.text)?;
    }

    if writer.offset > MAX_SNAPSHOT_LEN {
        return Err(WireError::TooLong);
    }
    Ok(writer.offset)
}

pub fn decode_snapshot(input: &[u8]) -> Result<HostSnapshot, WireError> {
    if input.len() > MAX_SNAPSHOT_LEN {
        return Err(WireError::TooLong);
    }
    let mut reader = Reader { input, offset: 0 };
    let version = reader.byte()?;
    if version != SNAPSHOT_VERSION {
        return Err(WireError::UnsupportedVersion(version));
    }

    let valid_for_secs = reader.u16()?;
    if valid_for_secs == 0 || valid_for_secs > MAX_VALIDITY_SECS {
        return Err(WireError::InvalidValidity(valid_for_secs));
    }
    let personality = personality(reader.byte()?)?;
    let detail = detail(reader.byte()?)?;
    let flags = reader.byte()?;
    if flags & !0x0f != 0 || flags & (1 << 1) != 0 && flags & (1 << 0) == 0 {
        return Err(WireError::InvalidFlags);
    }

    let link_count = reader.byte()?;
    let admitted_links = reader.byte()?;
    let queue_depth = reader.u16()?;
    let peer_count = usize::from(reader.byte()?);
    if peer_count > 3 {
        return Err(WireError::TooLong);
    }
    let peer_overflow = reader.byte()?;
    if detail == DetailPolicy::Minimal
        && (flags & (1 << 2) != 0 || peer_count > 0 || peer_overflow > 0)
    {
        return Err(WireError::PrivacyViolation);
    }

    let node = if flags & (1 << 2) != 0 {
        let name = reader.text()?;
        let mut address_tail = [0; 8];
        address_tail.copy_from_slice(reader.bytes(8)?);
        let mut fingerprint = [0; 16];
        fingerprint.copy_from_slice(reader.bytes(16)?);
        let role = reader.text()?;
        let uptime_secs = reader.u32()?;
        Some(NodeSummary {
            name,
            address_tail,
            fingerprint,
            role,
            uptime_secs,
        })
    } else {
        None
    };

    let mut peers = [None; 3];
    for slot in peers.iter_mut().take(peer_count) {
        *slot = Some(PeerSummary {
            name: reader.text()?,
            path: peer_path(reader.byte()?)?,
            age_secs: reader.u32()?,
        });
    }

    let event = if flags & (1 << 3) != 0 {
        Some(UiEvent {
            source: event_source(reader.byte()?)?,
            kind: event_kind(reader.byte()?)?,
            text: reader.text()?,
        })
    } else {
        None
    };

    if reader.offset != input.len() {
        return Err(WireError::TrailingBytes);
    }

    Ok(HostSnapshot {
        valid_for_secs,
        personality,
        detail,
        node,
        link_count,
        admitted_links,
        queue_depth,
        ifac: match flags & 0x03 {
            0 => IfacState::Unknown,
            1 => IfacState::Off,
            3 => IfacState::On,
            _ => return Err(WireError::InvalidFlags),
        },
        peers,
        peer_overflow,
        event,
    })
}

fn personality(value: u8) -> Result<Personality, WireError> {
    match value {
        0 => Ok(Personality::Phy),
        1 => Ok(Personality::Retinue),
        2 => Ok(Personality::RNode),
        3 => Ok(Personality::MeshCore),
        4 => Ok(Personality::Sennet),
        _ => Err(WireError::InvalidEnum),
    }
}

fn detail(value: u8) -> Result<DetailPolicy, WireError> {
    match value {
        0 => Ok(DetailPolicy::Minimal),
        1 => Ok(DetailPolicy::Named),
        _ => Err(WireError::InvalidEnum),
    }
}

fn peer_path(value: u8) -> Result<PeerPath, WireError> {
    match value {
        0 => Ok(PeerPath::Direct),
        1 => Ok(PeerPath::Via),
        _ => Err(WireError::InvalidEnum),
    }
}

fn event_source(value: u8) -> Result<EventSource, WireError> {
    match value {
        0 => Ok(EventSource::Local),
        1 => Ok(EventSource::Host),
        _ => Err(WireError::InvalidEnum),
    }
}

fn event_kind(value: u8) -> Result<EventKind, WireError> {
    match value {
        0 => Ok(EventKind::Info),
        1 => Ok(EventKind::Received),
        2 => Ok(EventKind::Transmitted),
        3 => Ok(EventKind::Delivered),
        4 => Ok(EventKind::Propagated),
        5 => Ok(EventKind::Failed),
        _ => Err(WireError::InvalidEnum),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> HostSnapshot {
        HostSnapshot {
            valid_for_secs: 30,
            personality: Personality::Retinue,
            detail: DetailPolicy::Named,
            node: Some(NodeSummary {
                name: Text::from_truncated("HERALD"),
                address_tail: [0x4c, 0x9f, 0x03, 0xaa, 0x77, 0xe2, 0xbd, 0x08],
                fingerprint: [0x5a; 16],
                role: Text::from_truncated("NODE"),
                uptime_secs: 12_345,
            }),
            link_count: 3,
            admitted_links: 2,
            queue_depth: 7,
            ifac: IfacState::On,
            peers: [
                Some(PeerSummary {
                    name: Text::from_truncated("ESQUIRE"),
                    path: PeerPath::Direct,
                    age_secs: 120,
                }),
                Some(PeerSummary {
                    name: Text::from_truncated("MARSHAL"),
                    path: PeerPath::Via,
                    age_secs: 3_600,
                }),
                None,
            ],
            peer_overflow: 1,
            event: Some(UiEvent {
                source: EventSource::Host,
                kind: EventKind::Delivered,
                text: Text::from_truncated("DIRECT DELIVERED"),
            }),
        }
    }

    #[test]
    fn named_snapshot_round_trips_without_allocation() {
        let expected = snapshot();
        let mut bytes = [0; MAX_SNAPSHOT_LEN];
        let length = encode_snapshot(&expected, &mut bytes).unwrap();
        assert_eq!(decode_snapshot(&bytes[..length]), Ok(expected));
    }

    #[test]
    fn decoder_rejects_unknown_truncated_and_trailing_data() {
        assert_eq!(
            decode_snapshot(&[SNAPSHOT_VERSION + 1]),
            Err(WireError::UnsupportedVersion(SNAPSHOT_VERSION + 1))
        );

        let expected = snapshot();
        let mut bytes = [0; MAX_SNAPSHOT_LEN];
        let length = encode_snapshot(&expected, &mut bytes).unwrap();
        assert_eq!(
            decode_snapshot(&bytes[..length - 1]),
            Err(WireError::Truncated)
        );

        bytes[length] = 0;
        assert_eq!(
            decode_snapshot(&bytes[..length + 1]),
            Err(WireError::TrailingBytes)
        );
    }

    #[test]
    fn decoder_rejects_invalid_utf8_and_oversized_peer_count() {
        let minimal = HostSnapshot {
            detail: DetailPolicy::Named,
            node: Some(NodeSummary {
                name: Text::from_truncated("A"),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut bytes = [0; MAX_SNAPSHOT_LEN];
        let length = encode_snapshot(&minimal, &mut bytes).unwrap();

        // Header is 12 bytes; byte 12 is node-name length and byte 13 is text.
        bytes[13] = 0xff;
        assert_eq!(
            decode_snapshot(&bytes[..length]),
            Err(WireError::InvalidText(TextError::InvalidUtf8))
        );

        let mut bytes = [0; 12];
        bytes[0] = SNAPSHOT_VERSION;
        bytes[1..3].copy_from_slice(&15_u16.to_le_bytes());
        bytes[10] = 4;
        assert_eq!(decode_snapshot(&bytes), Err(WireError::TooLong));
    }

    #[test]
    fn validity_is_bounded() {
        let mut snapshot = HostSnapshot {
            valid_for_secs: 0,
            ..Default::default()
        };
        let mut bytes = [0; MAX_SNAPSHOT_LEN];
        assert_eq!(
            encode_snapshot(&snapshot, &mut bytes),
            Err(WireError::InvalidValidity(0))
        );
        snapshot.valid_for_secs = 301;
        assert_eq!(
            encode_snapshot(&snapshot, &mut bytes),
            Err(WireError::InvalidValidity(301))
        );
    }

    #[test]
    fn minimal_snapshot_cannot_smuggle_named_state() {
        let mut snapshot = snapshot();
        snapshot.detail = DetailPolicy::Minimal;
        let mut bytes = [0; MAX_SNAPSHOT_LEN];
        assert_eq!(
            encode_snapshot(&snapshot, &mut bytes),
            Err(WireError::PrivacyViolation)
        );

        snapshot.node = None;
        snapshot.peers = [None; 3];
        snapshot.peer_overflow = 0;
        let length = encode_snapshot(&snapshot, &mut bytes).unwrap();
        bytes[5] |= 1 << 2;
        assert_eq!(
            decode_snapshot(&bytes[..length]),
            Err(WireError::PrivacyViolation)
        );
    }
}
