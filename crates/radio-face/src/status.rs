use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextError {
    TooLong,
    NonAscii,
    InvalidUtf8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn try_from_str(value: &str) -> Result<Self, TextError> {
        if !value.is_ascii() {
            return Err(TextError::NonAscii);
        }
        if value.len() > N {
            return Err(TextError::TooLong);
        }
        let mut text = Self::empty();
        text.bytes[..value.len()].copy_from_slice(value.as_bytes());
        text.len = value.len();
        Ok(text)
    }

    pub fn try_from_bytes(value: &[u8]) -> Result<Self, TextError> {
        let value = core::str::from_utf8(value).map_err(|_| TextError::InvalidUtf8)?;
        Self::try_from_str(value)
    }

    pub fn from_truncated(value: &str) -> Self {
        let mut text = Self::empty();
        for byte in value.bytes() {
            if text.len == N {
                break;
            }
            text.bytes[text.len] = if byte.is_ascii() { byte } else { b'?' };
            text.len += 1;
        }
        text
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len])
            .expect("Text constructors preserve ASCII UTF-8")
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<const N: usize> fmt::Debug for Text<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Text").field(&self.as_str()).finish()
    }
}

impl<const N: usize> fmt::Display for Text<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<const N: usize> fmt::Write for Text<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if !value.is_ascii() || self.len + value.len() > N {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + value.len()].copy_from_slice(value.as_bytes());
        self.len += value.len();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum RadioState {
    #[default]
    Booting = 0,
    Online = 1,
    Fault = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HostState {
    #[default]
    Detached = 0,
    Attached = 1,
    Fault = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum PowerSource {
    #[default]
    Unknown = 0,
    Usb = 1,
    Battery = 2,
    Solar = 3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum SleepState {
    #[default]
    Disabled = 0,
    Awake = 1,
    Armed = 2,
    Sleeping = 3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum WakeSource {
    #[default]
    Unknown = 0,
    Button = 1,
    Host = 2,
    Radio = 3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadioProfile {
    pub frequency_hz: Option<u32>,
    pub bandwidth_hz: Option<u32>,
    pub spreading_factor: Option<u8>,
    pub coding_rate_denominator: Option<u8>,
    pub tx_power_dbm: Option<i8>,
    pub sync_word: Option<u8>,
    pub name: Text<16>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxSummary {
    pub frame_len: u16,
    pub rssi_dbm: i16,
    pub snr_tenths_db: i16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TxResult {
    #[default]
    None,
    Sent {
        frame_len: u16,
    },
    Failed {
        code: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub code: u8,
    pub message: Text<24>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalStatus {
    pub board: Text<16>,
    pub firmware: Text<12>,
    pub uptime_secs: u32,
    pub radio: RadioState,
    pub host: HostState,
    pub power_source: PowerSource,
    pub battery_percent: Option<u8>,
    pub millivolts: Option<u16>,
    pub display_on: bool,
    pub sleep: SleepState,
    pub last_wake: WakeSource,
    pub profile: RadioProfile,
    pub tx_frames: u32,
    pub rx_frames: u32,
    pub last_rx: Option<RxSummary>,
    pub last_tx: TxResult,
    pub fault: Option<Fault>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Personality {
    #[default]
    Phy = 0,
    Retinue = 1,
    RNode = 2,
    MeshCore = 3,
    Sennet = 4,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum DetailPolicy {
    #[default]
    Minimal = 0,
    Named = 1,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum IfacState {
    #[default]
    Unknown = 0,
    Off = 1,
    On = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeSummary {
    pub name: Text<16>,
    pub address_tail: [u8; 8],
    pub fingerprint: [u8; 16],
    pub role: Text<12>,
    pub uptime_secs: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum PeerPath {
    #[default]
    Direct = 0,
    Via = 1,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerSummary {
    pub name: Text<12>,
    pub path: PeerPath,
    pub age_secs: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum EventSource {
    #[default]
    Local = 0,
    Host = 1,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum EventKind {
    #[default]
    Info = 0,
    Received = 1,
    Transmitted = 2,
    Delivered = 3,
    Propagated = 4,
    Failed = 5,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiEvent {
    pub source: EventSource,
    pub kind: EventKind,
    pub text: Text<24>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostSnapshot {
    pub valid_for_secs: u16,
    pub personality: Personality,
    pub detail: DetailPolicy,
    pub node: Option<NodeSummary>,
    pub link_count: u8,
    pub admitted_links: u8,
    pub queue_depth: u16,
    pub ifac: IfacState,
    pub peers: [Option<PeerSummary>; 3],
    pub peer_overflow: u8,
    pub event: Option<UiEvent>,
}

impl HostSnapshot {
    pub const fn is_fresh(&self, elapsed_secs: u32) -> bool {
        elapsed_secs < self.valid_for_secs as u32
    }

    pub fn peer_count(&self) -> usize {
        self.peers.iter().flatten().count()
    }

    pub fn named_node(&self) -> Option<&NodeSummary> {
        if self.detail == DetailPolicy::Named {
            self.node.as_ref()
        } else {
            None
        }
    }
}

impl Default for HostSnapshot {
    fn default() -> Self {
        Self {
            valid_for_secs: 15,
            personality: Personality::Phy,
            detail: DetailPolicy::Minimal,
            node: None,
            link_count: 0,
            admitted_links: 0,
            queue_depth: 0,
            ifac: IfacState::Unknown,
            peers: [None; 3],
            peer_overflow: 0,
            event: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write as _;

    #[test]
    fn bounded_text_rejects_ambiguous_display_input() {
        assert_eq!(Text::<4>::try_from_str("fiver"), Err(TextError::TooLong));
        assert_eq!(Text::<8>::try_from_str("naïve"), Err(TextError::NonAscii));
        assert_eq!(
            Text::<8>::try_from_bytes(&[0xff]),
            Err(TextError::InvalidUtf8)
        );
    }

    #[test]
    fn bounded_text_supports_allocation_free_formatting() {
        let mut text = Text::<12>::empty();
        write!(&mut text, "{} / {}", 12, 34).unwrap();
        assert_eq!(text.as_str(), "12 / 34");
    }

    #[test]
    fn host_truth_expires_and_minimal_policy_hides_identity() {
        let mut snapshot = HostSnapshot {
            valid_for_secs: 5,
            node: Some(NodeSummary {
                name: Text::from_truncated("HERALD"),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(snapshot.is_fresh(4));
        assert!(!snapshot.is_fresh(5));
        assert!(snapshot.named_node().is_none());
        snapshot.detail = DetailPolicy::Named;
        assert_eq!(snapshot.named_node().unwrap().name.as_str(), "HERALD");
    }
}
