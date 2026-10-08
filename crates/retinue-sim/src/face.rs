//! What each node's own screen shows: the trace's [`NodeState`]s as radio-face status, and the
//! face track that carries them. Feature `face`.
//!
//! [`face`] is the one mapping from a [`NodeState`] to radio-face's [`LocalStatus`] and
//! [`HostSnapshot`], so a consumer drawing a node's face does not re-derive it. It fills the
//! fields the Retinue channel node fills from the same facts (`radio-hand/src/channel/node.rs`
//! and `face.rs`), and leaves every other field at radio-face's default:
//!
//! | [`NodeState`] | radio-face |
//! | --- | --- |
//! | `tx_frames`, `rx_frames` | `LocalStatus.tx_frames`, `.rx_frames` |
//! | `last_tx_len` | `LocalStatus.last_tx = TxResult::Sent { frame_len }` |
//! | `last_rx_len` | `LocalStatus.last_rx.frame_len`, with zero RSSI and SNR |
//! | `links` | `HostSnapshot.link_count` and `.admitted_links`, saturating at 255 |
//! | `event` | `HostSnapshot.event`, from `EventSource::Local`, truncated to 24 ASCII bytes |
//! | (always) | `HostSnapshot.personality = Personality::Retinue` |
//!
//! The medium simulates no signal, so RSSI and SNR are zero. It has no unsent queue, so
//! `queue_depth` is zero. The trace carries no board, firmware, power, profile or uptime, and
//! no peer names or ages, so the board-owned `LocalStatus` fields, `HostSnapshot.node`,
//! `.peers` and `.ifac` stay at their defaults.
//!
//! # The face track, schema `retinue-sim.face-track/v1`
//!
//! A file derived from one route trace, kept separate from it so the route trace's schema is
//! unchanged. [`FaceTrack::to_json`] is the canonical form: compact JSON, fields in
//! declaration order. The top level is
//!
//! - `schema`: [`FACE_SCHEMA`];
//! - `route_trace`: the schema of the trace it was derived from, [`crate::SCHEMA`];
//! - `trace_sha256`: the SHA-256 of that trace's canonical JSON ([`Trace::to_json`]), as
//!   lowercase hex, so a consumer can check it holds the matching pair. The lab example
//!   prints each file with a trailing newline, which the digest does not cover;
//! - `scenario`: the trace's scenario name;
//! - `entries`: one per trace event that carries a [`NodeState`] (`transmit`, `receive` and
//!   `link_request_expired`), in trace order.
//!
//! Each entry has `event` (the index into the trace's `events`), `t`, `node`, and that node's
//! face after the event as two documents in radio-mirror's input shape: `local`, schema
//! `radio-mirror.local/v1`, and `host`, schema `radio-mirror.host/v1`. Serialized on their
//! own, they are what radio-mirror's `set_local_json` and `set_host_json` accept. A node's face
//! at step *i* is its latest entry at or before *i*; before its first entry it is
//! radio-face's default.
//!
//! The host document carries `valid_for_secs` as the channel node publishes it (15 s), and
//! the channel republishes on every beat. A player that ages host snapshots in simulated time
//! re-sets the node's latest host document as it advances, as the board would receive it.
//!
//! A change that would break a reader of v1 takes a new schema id.

use radio_face::{
    EventKind, EventSource, HostSnapshot, LocalStatus, Personality, RxSummary, Text, TxResult,
    UiEvent,
};
use serde::{Deserialize, Serialize};

use crate::trace::{Event, FaceEventKind, NodeState, Trace};

/// The schema id every face track carries. A breaking change takes a new version.
pub const FACE_SCHEMA: &str = "retinue-sim.face-track/v1";
/// The `schema` of radio-mirror's local status document.
pub const LOCAL_SCHEMA: &str = "radio-mirror.local/v1";
/// The `schema` of radio-mirror's host snapshot document.
pub const HOST_SCHEMA: &str = "radio-mirror.host/v1";

/// One node's face: the firmware-owned status and the host snapshot its screen draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Face {
    pub local: LocalStatus,
    pub host: HostSnapshot,
}

/// A node's face after an event. See the module docs for the mapping.
pub fn face(state: &NodeState) -> Face {
    let local = LocalStatus {
        tx_frames: state.tx_frames,
        rx_frames: state.rx_frames,
        last_tx: state
            .last_tx_len
            .map_or(TxResult::None, |frame_len| TxResult::Sent { frame_len }),
        last_rx: state.last_rx_len.map(|frame_len| RxSummary {
            frame_len,
            ..RxSummary::default()
        }),
        ..LocalStatus::default()
    };
    let links = u8::try_from(state.links).unwrap_or(u8::MAX);
    let host = HostSnapshot {
        personality: Personality::Retinue,
        link_count: links,
        admitted_links: links,
        event: state.event.as_ref().map(|event| UiEvent {
            source: EventSource::Local,
            kind: event_kind(event.kind),
            text: Text::from_truncated(&event.text),
        }),
        ..HostSnapshot::default()
    };
    Face { local, host }
}

fn event_kind(kind: FaceEventKind) -> EventKind {
    match kind {
        FaceEventKind::Info => EventKind::Info,
        FaceEventKind::Received => EventKind::Received,
        FaceEventKind::Transmitted => EventKind::Transmitted,
        FaceEventKind::Delivered => EventKind::Delivered,
        FaceEventKind::Propagated => EventKind::Propagated,
        FaceEventKind::Failed => EventKind::Failed,
    }
}

/// The faces of one route trace, step by step. See the module docs for the schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceTrack {
    pub schema: String,
    pub route_trace: String,
    pub trace_sha256: String,
    pub scenario: String,
    pub entries: Vec<FaceEntry>,
}

impl FaceTrack {
    /// Every state-carrying event's face, in trace order.
    pub fn from_trace(trace: &Trace) -> Self {
        let entries = trace
            .events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                let (t, node, state) = match event {
                    Event::Transmit { t, node, state, .. }
                    | Event::Receive { t, node, state, .. }
                    | Event::LinkRequestExpired { t, node, state, .. } => (*t, node, state),
                    Event::Cut { .. }
                    | Event::Send { .. }
                    | Event::SendRefused { .. }
                    | Event::Delivered { .. } => return None,
                };
                let Face { local, host } = face(state);
                Some(FaceEntry {
                    event: u32::try_from(index).expect("fewer than 2^32 events"),
                    t,
                    node: node.clone(),
                    local: LocalDocument::from(&local),
                    host: HostDocument::from(&host),
                })
            })
            .collect();
        let digest = retinue::hash::full_hash(trace.to_json().as_bytes());
        Self {
            schema: FACE_SCHEMA.to_owned(),
            route_trace: trace.schema.clone(),
            trace_sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            scenario: trace.scenario.clone(),
            entries,
        }
    }

    /// The canonical serialization: compact JSON, fields in declaration order.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("face track types serialize")
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }
}

/// One node's face after one trace event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceEntry {
    /// The index of the event in the route trace's `events`.
    pub event: u32,
    pub t: u64,
    pub node: String,
    pub local: LocalDocument,
    pub host: HostDocument,
}

/// radio-mirror's local status document, holding the fields [`face`] fills. The rest are
/// absent, which radio-mirror reads as radio-face's defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDocument {
    pub schema: String,
    pub tx_frames: u32,
    pub rx_frames: u32,
    pub last_rx: Option<RxDocument>,
    pub last_tx: TxDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RxDocument {
    pub frame_len: u16,
    pub rssi_dbm: i16,
    pub snr_tenths_db: i16,
}

/// radio-face's `TxResult` as radio-mirror spells it: `"none"`, `{"sent":{"frame_len":n}}` or
/// `{"failed":{"code":n}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum TxDocument {
    None,
    Sent { frame_len: u16 },
    Failed { code: u8 },
}

/// radio-mirror's host snapshot document, holding the fields [`face`] fills. The rest are
/// absent, which radio-mirror reads as radio-face's defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostDocument {
    pub schema: String,
    pub valid_for_secs: u16,
    /// radio-face's `Personality`, lowercase.
    pub personality: String,
    pub link_count: u8,
    pub admitted_links: u8,
    pub queue_depth: u16,
    pub event: Option<EventDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDocument {
    /// radio-face's `EventSource`, lowercase.
    pub source: String,
    /// radio-face's `EventKind`, lowercase.
    pub kind: String,
    pub text: String,
}

impl From<&LocalStatus> for LocalDocument {
    fn from(local: &LocalStatus) -> Self {
        Self {
            schema: LOCAL_SCHEMA.to_owned(),
            tx_frames: local.tx_frames,
            rx_frames: local.rx_frames,
            last_rx: local.last_rx.map(|rx| RxDocument {
                frame_len: rx.frame_len,
                rssi_dbm: rx.rssi_dbm,
                snr_tenths_db: rx.snr_tenths_db,
            }),
            last_tx: match local.last_tx {
                TxResult::None => TxDocument::None,
                TxResult::Sent { frame_len } => TxDocument::Sent { frame_len },
                TxResult::Failed { code } => TxDocument::Failed { code },
            },
        }
    }
}

impl From<&HostSnapshot> for HostDocument {
    fn from(host: &HostSnapshot) -> Self {
        Self {
            schema: HOST_SCHEMA.to_owned(),
            valid_for_secs: host.valid_for_secs,
            personality: match host.personality {
                Personality::Phy => "phy",
                Personality::Retinue => "retinue",
                Personality::RNode => "rnode",
                Personality::MeshCore => "meshcore",
                Personality::Sennet => "sennet",
            }
            .to_owned(),
            link_count: host.link_count,
            admitted_links: host.admitted_links,
            queue_depth: host.queue_depth,
            event: host.event.map(|event| EventDocument {
                source: match event.source {
                    EventSource::Local => "local",
                    EventSource::Host => "host",
                }
                .to_owned(),
                kind: match event.kind {
                    EventKind::Info => "info",
                    EventKind::Received => "received",
                    EventKind::Transmitted => "transmitted",
                    EventKind::Delivered => "delivered",
                    EventKind::Propagated => "propagated",
                    EventKind::Failed => "failed",
                }
                .to_owned(),
                text: event.text.as_str().to_owned(),
            }),
        }
    }
}
