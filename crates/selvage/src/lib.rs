#![forbid(unsafe_code)]
#![no_std]

mod command;
pub mod excursion_stream;
pub mod kiss;
pub mod observation;
pub mod personality;
mod profile;
mod snapshot;
#[cfg(test)]
mod tests;
mod wire;

pub use command::{CommandEvent, CommandKind, CommandStream};
pub use excursion_stream::{ExcursionByte, ExcursionStream};
pub use observation::{CMD_OBSERVATION, EVENT_OBSERVATION};
pub use profile::{
    PhyProfile, ProfileError, decode_config_command, decode_excursion_command,
    encode_config_command, sx126x_sync_word,
};
pub use snapshot::{UiSnapshotWireError, decode_ui_snapshot_command, encode_ui_snapshot_command};
pub use wire::{
    CMD_CONFIG, CMD_EXCURSION, CMD_TX, CMD_UI_SNAPSHOT, CONFIG_ACCEPTED, CONFIG_COMMAND_LEN,
    CONFIG_MALFORMED, CONFIG_OUT_OF_REGION, CONFIG_RADIO_FAULT, CONFIG_UNSUPPORTED, EVENT_CONFIG,
    EVENT_DIAGNOSTIC, EVENT_EXCURSION, EVENT_RX, EVENT_TX, EVENT_UI_SNAPSHOT,
    EXCURSION_COMMAND_LEN, MAX_COMMAND_LEN, MAX_RADIO_FRAME_LEN, MAX_UI_SNAPSHOT_COMMAND_BODY_LEN,
    MAX_UI_SNAPSHOT_COMMAND_LEN, MAX_UI_SNAPSHOT_LEN, MESHCORE_SYNC_WORD, MESHTASTIC_SYNC_WORD,
    TX_ACCEPTED, TX_CHANNEL_BUSY, TX_NO_REGION, TX_OVER_DUTY, TX_RADIO_FAULT, TX_TIMEOUT,
    TX_TOO_LONG, TX_UNKNOWN_COMMAND, UI_SNAPSHOT_ACCEPTED, UI_SNAPSHOT_MALFORMED,
    UI_SNAPSHOT_TOO_LONG, UI_SNAPSHOT_UNSUPPORTED_VERSION, WAKE_BYTE,
};
