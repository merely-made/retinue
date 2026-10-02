//! JSON documents for `LocalStatus` and `HostSnapshot`.
//!
//! Each document names its version in a required `schema` field,
//! [`LOCAL_SCHEMA`] or [`HOST_SCHEMA`]; any other value is refused.
//!
//! The definitions below mirror radio-face's types field for field (serde
//! *remote* derives, and exhaustive conversions for the two documents), so a
//! field added to or removed from radio-face fails this crate's build rather
//! than drifting silently. Every other field is optional and defaults as in
//! radio-face; unknown fields are refused. Text must fit and be ASCII.
//! A host document is passed through radio-face's own wire codec, so the mirror
//! only shows what a real radio could be told.

use alloc::{format, string::String, vec::Vec};
use core::fmt;

use radio_face::{
    DetailPolicy, EventKind, EventSource, Fault, GnssFix, GnssState, HostSnapshot, HostState,
    IfacState, LocalStatus, MAX_SNAPSHOT_LEN, NodeSummary, PeerPath, PeerSummary, Personality,
    PowerSource, RadioProfile, RadioState, RxSummary, SleepState, Text, TxResult, UiEvent,
    WakeSource, WireError, decode_snapshot, encode_snapshot,
};
use serde::{Deserialize, Deserializer, de::Error as _};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    Json(String),
    Wire(WireError),
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(message) => write!(f, "status document: {message}"),
            Self::Wire(error) => write!(f, "host snapshot refused by the radio wire: {error:?}"),
        }
    }
}

impl core::error::Error for InputError {}

/// The `schema` of a local status document.
pub const LOCAL_SCHEMA: &str = "radio-mirror.local/v1";
/// The `schema` of a host snapshot document.
pub const HOST_SCHEMA: &str = "radio-mirror.host/v1";

pub fn local_from_json(json: &str) -> Result<LocalStatus, InputError> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let local = LocalDocument::deserialize(&mut deserializer).map_err(json_error)?;
    deserializer.end().map_err(json_error)?;
    Ok(local.into())
}

/// Parses a host document and passes it through `encode_snapshot`/`decode_snapshot`.
pub fn host_from_json(json: &str) -> Result<HostSnapshot, InputError> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let host = HostDocument::deserialize(&mut deserializer).map_err(json_error)?;
    deserializer.end().map_err(json_error)?;
    through_wire(&host.into())
}

fn schema<'de, D>(deserializer: D, expected: &'static str) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    let found = String::deserialize(deserializer)
        .map_err(|error| D::Error::custom(format!("schema: {error}")))?;
    if found == expected {
        Ok(())
    } else {
        Err(D::Error::custom(format!(
            "schema {found:?} is not supported (expected {expected:?})"
        )))
    }
}

fn local_schema<'de, D: Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    schema(deserializer, LOCAL_SCHEMA)
}

fn host_schema<'de, D: Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    schema(deserializer, HOST_SCHEMA)
}

pub fn through_wire(host: &HostSnapshot) -> Result<HostSnapshot, InputError> {
    let mut bytes = [0; MAX_SNAPSHOT_LEN];
    let len = encode_snapshot(host, &mut bytes).map_err(InputError::Wire)?;
    decode_snapshot(&bytes[..len]).map_err(InputError::Wire)
}

fn json_error(error: serde_json::Error) -> InputError {
    InputError::Json(format!("{error}"))
}

fn text<'de, D, const N: usize>(deserializer: D) -> Result<Text<N>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Text::try_from_str(&value)
        .map_err(|error| D::Error::custom(format!("text {value:?} ({N} ASCII max): {error:?}")))
}

fn hex<'de, D, const N: usize>(deserializer: D) -> Result<[u8; N], D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let invalid = || D::Error::custom(format!("expected {N} bytes as hex, got {value:?}"));
    if value.len() != N * 2 || !value.is_ascii() {
        return Err(invalid());
    }
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok(bytes)
}

/// `Option<T>` through a remote definition.
macro_rules! optional {
    ($name:ident, $remote:ty, $def:literal) => {
        fn $name<'de, D>(deserializer: D) -> Result<Option<$remote>, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[derive(Deserialize)]
            struct Wrap(#[serde(with = $def)] $remote);
            Ok(Option::<Wrap>::deserialize(deserializer)?.map(|Wrap(value)| value))
        }
    };
}

optional!(optional_rx, RxSummary, "RxSummaryDef");
optional!(optional_fault, Fault, "FaultDef");
optional!(optional_node, NodeSummary, "NodeSummaryDef");
optional!(optional_event, UiEvent, "UiEventDef");

/// Up to three peers as a list; the firmware holds three slots.
fn peers<'de, D>(deserializer: D) -> Result<[Option<PeerSummary>; 3], D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Wrap(#[serde(with = "PeerSummaryDef")] PeerSummary);
    let list = Vec::<Wrap>::deserialize(deserializer)?;
    if list.len() > 3 {
        return Err(D::Error::custom(
            "at most 3 peers; count the rest in peer_overflow",
        ));
    }
    let mut slots = [None; 3];
    for (slot, Wrap(peer)) in slots.iter_mut().zip(list) {
        *slot = Some(peer);
    }
    Ok(slots)
}

macro_rules! unit_enum {
    ($def:ident, $remote:literal { $($variant:ident),+ $(,)? }) => {
        #[derive(Deserialize)]
        #[serde(remote = $remote, rename_all = "lowercase")]
        enum $def { $($variant),+ }
    };
}

unit_enum!(RadioStateDef, "RadioState" { Booting, Online, Fault });
unit_enum!(HostStateDef, "HostState" { Detached, Attached, Fault });
unit_enum!(PowerSourceDef, "PowerSource" { Unknown, Usb, Battery, Solar });
unit_enum!(SleepStateDef, "SleepState" { Disabled, Awake, Armed, Sleeping });
unit_enum!(WakeSourceDef, "WakeSource" { Unknown, Button, Host, Radio });
unit_enum!(PersonalityDef, "Personality" { Phy, Retinue, RNode, MeshCore, Sennet });
unit_enum!(DetailPolicyDef, "DetailPolicy" { Minimal, Named });
unit_enum!(IfacStateDef, "IfacState" { Unknown, Off, On });
unit_enum!(PeerPathDef, "PeerPath" { Direct, Via });
unit_enum!(EventSourceDef, "EventSource" { Local, Host });
unit_enum!(EventKindDef, "EventKind" { Info, Received, Transmitted, Delivered, Propagated, Failed });

#[derive(Deserialize)]
#[serde(remote = "TxResult", rename_all = "kebab-case", deny_unknown_fields)]
enum TxResultDef {
    None,
    Sent { frame_len: u16 },
    Failed { code: u8 },
}

#[derive(Deserialize)]
#[serde(remote = "GnssFix", deny_unknown_fields)]
struct GnssFixDef {
    lat_e7: i32,
    lon_e7: i32,
    satellites: u8,
    hdop_tenths: u16,
    at_uptime_secs: u32,
}

#[derive(Deserialize)]
#[serde(remote = "GnssState", rename_all = "kebab-case")]
enum GnssStateDef {
    Absent,
    NoFix,
    Fix(#[serde(with = "GnssFixDef")] GnssFix),
}

#[derive(Deserialize)]
#[serde(remote = "RadioProfile", deny_unknown_fields)]
struct RadioProfileDef {
    #[serde(default)]
    frequency_hz: Option<u32>,
    #[serde(default)]
    bandwidth_hz: Option<u32>,
    #[serde(default)]
    spreading_factor: Option<u8>,
    #[serde(default)]
    coding_rate_denominator: Option<u8>,
    #[serde(default)]
    tx_power_dbm: Option<i8>,
    #[serde(default)]
    sync_word: Option<u8>,
    #[serde(default, deserialize_with = "text")]
    name: Text<16>,
}

#[derive(Deserialize)]
#[serde(remote = "RxSummary", deny_unknown_fields)]
struct RxSummaryDef {
    frame_len: u16,
    rssi_dbm: i16,
    snr_tenths_db: i16,
}

#[derive(Deserialize)]
#[serde(remote = "Fault", deny_unknown_fields)]
struct FaultDef {
    code: u8,
    #[serde(deserialize_with = "text")]
    message: Text<24>,
}

/// A `LocalStatus` with its `schema`. Not a remote derive, since the document
/// has a field the type lacks; the exhaustive conversion below keeps the pin.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalDocument {
    #[serde(deserialize_with = "local_schema")]
    schema: (),
    #[serde(default, deserialize_with = "text")]
    board: Text<16>,
    #[serde(default, deserialize_with = "text")]
    firmware: Text<12>,
    #[serde(default)]
    uptime_secs: u32,
    #[serde(default, with = "RadioStateDef")]
    radio: RadioState,
    #[serde(default, with = "HostStateDef")]
    host: HostState,
    #[serde(default, with = "PowerSourceDef")]
    power_source: PowerSource,
    #[serde(default)]
    battery_percent: Option<u8>,
    #[serde(default)]
    millivolts: Option<u16>,
    #[serde(default)]
    display_on: bool,
    #[serde(default, with = "SleepStateDef")]
    sleep: SleepState,
    #[serde(default, with = "WakeSourceDef")]
    last_wake: WakeSource,
    #[serde(default, with = "RadioProfileDef")]
    profile: RadioProfile,
    #[serde(default)]
    tx_frames: u32,
    #[serde(default)]
    rx_frames: u32,
    #[serde(default, deserialize_with = "optional_rx")]
    last_rx: Option<RxSummary>,
    #[serde(default, with = "TxResultDef")]
    last_tx: TxResult,
    #[serde(default, deserialize_with = "optional_fault")]
    fault: Option<Fault>,
    #[serde(default, with = "GnssStateDef")]
    gnss: GnssState,
}

impl From<LocalDocument> for LocalStatus {
    fn from(document: LocalDocument) -> Self {
        let LocalDocument {
            schema: (),
            board,
            firmware,
            uptime_secs,
            radio,
            host,
            power_source,
            battery_percent,
            millivolts,
            display_on,
            sleep,
            last_wake,
            profile,
            tx_frames,
            rx_frames,
            last_rx,
            last_tx,
            fault,
            gnss,
        } = document;
        Self {
            board,
            firmware,
            uptime_secs,
            radio,
            host,
            power_source,
            battery_percent,
            millivolts,
            display_on,
            sleep,
            last_wake,
            profile,
            tx_frames,
            rx_frames,
            last_rx,
            last_tx,
            fault,
            gnss,
        }
    }
}

#[derive(Deserialize)]
#[serde(remote = "NodeSummary", deny_unknown_fields)]
struct NodeSummaryDef {
    #[serde(default, deserialize_with = "text")]
    name: Text<16>,
    #[serde(default, deserialize_with = "hex")]
    address_tail: [u8; 8],
    #[serde(default, deserialize_with = "hex")]
    fingerprint: [u8; 16],
    #[serde(default, deserialize_with = "text")]
    role: Text<12>,
    #[serde(default)]
    uptime_secs: u32,
}

#[derive(Deserialize)]
#[serde(remote = "PeerSummary", deny_unknown_fields)]
struct PeerSummaryDef {
    #[serde(default, deserialize_with = "text")]
    name: Text<12>,
    #[serde(default, with = "PeerPathDef")]
    path: PeerPath,
    #[serde(default)]
    age_secs: u32,
}

#[derive(Deserialize)]
#[serde(remote = "UiEvent", deny_unknown_fields)]
struct UiEventDef {
    #[serde(default, with = "EventSourceDef")]
    source: EventSource,
    #[serde(default, with = "EventKindDef")]
    kind: EventKind,
    #[serde(default, deserialize_with = "text")]
    text: Text<24>,
}

fn default_validity() -> u16 {
    HostSnapshot::default().valid_for_secs
}

/// A `HostSnapshot` with its `schema`, as [`LocalDocument`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostDocument {
    #[serde(deserialize_with = "host_schema")]
    schema: (),
    #[serde(default = "default_validity")]
    valid_for_secs: u16,
    #[serde(default, with = "PersonalityDef")]
    personality: Personality,
    #[serde(default, with = "DetailPolicyDef")]
    detail: DetailPolicy,
    #[serde(default, deserialize_with = "optional_node")]
    node: Option<NodeSummary>,
    #[serde(default)]
    link_count: u8,
    #[serde(default)]
    admitted_links: u8,
    #[serde(default)]
    queue_depth: u16,
    #[serde(default, with = "IfacStateDef")]
    ifac: IfacState,
    #[serde(default, deserialize_with = "peers")]
    peers: [Option<PeerSummary>; 3],
    #[serde(default)]
    peer_overflow: u8,
    #[serde(default, deserialize_with = "optional_event")]
    event: Option<UiEvent>,
}

impl From<HostDocument> for HostSnapshot {
    fn from(document: HostDocument) -> Self {
        let HostDocument {
            schema: (),
            valid_for_secs,
            personality,
            detail,
            node,
            link_count,
            admitted_links,
            queue_depth,
            ifac,
            peers,
            peer_overflow,
            event,
        } = document;
        Self {
            valid_for_secs,
            personality,
            detail,
            node,
            link_count,
            admitted_links,
            queue_depth,
            ifac,
            peers,
            peer_overflow,
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document with the current schema and `rest` (`,"field":value...`).
    fn local(rest: &str) -> String {
        format!(r#"{{"schema":"{LOCAL_SCHEMA}"{rest}}}"#)
    }

    fn host(rest: &str) -> String {
        format!(r#"{{"schema":"{HOST_SCHEMA}"{rest}}}"#)
    }

    #[test]
    fn documents_without_the_current_schema_are_refused() {
        for json in [
            "{}".into(),
            r#"{"schema":"radio-mirror.local/v2"}"#.into(),
            format!(r#"{{"schema":"{HOST_SCHEMA}"}}"#),
            r#"{"schema":1}"#.into(),
        ] {
            let error = local_from_json(&json).unwrap_err();
            assert!(
                matches!(&error, InputError::Json(message) if message.contains("schema")),
                "{json}: {error}"
            );
        }
        for json in [
            "{}".into(),
            r#"{"schema":"radio-mirror.host/v0"}"#.into(),
            format!(r#"{{"schema":"{LOCAL_SCHEMA}"}}"#),
        ] {
            let error = host_from_json(&json).unwrap_err();
            assert!(
                matches!(&error, InputError::Json(message) if message.contains("schema")),
                "{json}: {error}"
            );
        }
    }

    #[test]
    fn empty_documents_are_radio_face_defaults() {
        assert_eq!(local_from_json(&local("")).unwrap(), LocalStatus::default());
        assert_eq!(host_from_json(&host("")).unwrap(), HostSnapshot::default());
    }

    #[test]
    fn unknown_fields_and_long_text_are_refused() {
        assert!(matches!(
            local_from_json(&local(r#","route":"x""#)),
            Err(InputError::Json(_))
        ));
        assert!(matches!(
            local_from_json(&local(r#","board":"A BOARD NAME TOO LONG""#)),
            Err(InputError::Json(_))
        ));
        assert!(matches!(
            host_from_json(&host(r#","peers":[{},{},{},{}]"#)),
            Err(InputError::Json(_))
        ));
    }

    #[test]
    fn host_documents_obey_the_wire_privacy_rule() {
        let named_under_minimal = host(r#","detail":"minimal","node":{"name":"HERALD"}"#);
        assert_eq!(
            host_from_json(&named_under_minimal),
            Err(InputError::Wire(WireError::PrivacyViolation))
        );
        assert_eq!(
            host_from_json(&host(r#","valid_for_secs":0"#)),
            Err(InputError::Wire(WireError::InvalidValidity(0)))
        );
    }

    #[test]
    fn tagged_values_parse() {
        let local = local_from_json(&local(
            r#","last_tx":{"sent":{"frame_len":247}},"gnss":"no-fix","radio":"online""#,
        ))
        .unwrap();
        assert_eq!(local.last_tx, TxResult::Sent { frame_len: 247 });
        assert_eq!(local.gnss, GnssState::NoFix);
        assert_eq!(local.radio, RadioState::Online);
    }
}
