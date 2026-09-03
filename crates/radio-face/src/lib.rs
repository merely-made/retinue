#![no_std]
#![forbid(unsafe_code)]

//! Truthful, bounded on-device UI state for small radios.
//!
//! The crate combines firmware-owned [`LocalStatus`] with an optional,
//! explicitly lossy [`HostSnapshot`]. It has no Retinue, Sennet, MeshCore, or
//! transport domain types.

#[cfg(test)]
extern crate std;

pub mod controller;
pub mod render;
pub mod status;
pub mod wire;

pub use controller::{
    Action, Button, CHORD_PRESS_MS, Controller, InputEvent, InputProfile, LONG_PRESS_MS, LedIntent,
    LedSignal, MenuItem, Page, PressClassifier, Screen, led_intent,
};
pub use render::{Surface, Theme, render};
pub use status::{
    DetailPolicy, EventKind, EventSource, Fault, GnssFix, GnssState, HostSnapshot, HostState,
    IfacState, LocalStatus, NodeSummary, PeerPath, PeerSummary, Personality, PowerSource,
    RadioProfile, RadioState, RxSummary, SleepState, Text, TextError, TxResult, UiEvent,
    WakeSource,
};
pub use wire::{
    MAX_SNAPSHOT_LEN, MAX_VALIDITY_SECS, SNAPSHOT_VERSION, WireError, decode_snapshot,
    encode_snapshot,
};
