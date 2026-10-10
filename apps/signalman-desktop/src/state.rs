//! The desktop face's own state.
//!
//! Everything here is presentation and selection. Package trust, compatibility,
//! plans, execution, and recovery decisions stay behind Signalman and Linkboy —
//! this type cannot construct, alter, or execute a `FlashPlan`, and the only
//! way it advances the owner flow is by calling the flow's own methods.
//!
//! It is deliberately free of I/O. Device surveys and the executor run
//! elsewhere and hand their results in, which is what lets the whole six-page
//! flow be driven in a headless test with no board plugged in.

mod firmware;
mod messages;
mod network;
mod observation;
mod voice;

use std::time::Duration;

use linkboy::BoardFamily;
use seiche::{LayoutSnapshot, NodeKey};
use signalman::management::ManagementRelationId;
use signalman::message::{MessageId, MessagePeer};
use signalman::voice::{DecodedVoice, VoiceEncoding};
use signalman::{DeviceCandidate, FirmwareCatalog, FirmwareInstallRecovery, FirmwareInstaller};

use crate::audio::{AudioDeviceChoice, PlaybackReceipt};
use crate::availability::AvailabilityCapture;
use crate::device_mere::DeviceMere;
use crate::messages::MessageStore;
use crate::network::NetworkInput;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DesktopSection {
    #[default]
    Devices,
    Network,
    Messages,
    Radio,
    Map,
    Browse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationRequest {
    Load,
    Export,
    SaveSettings,
    StartCollector { port: String, association: String },
    StopCollector,
}

/// The pinned Cambium canvas has one honest label-density seam: labels shown
/// or hidden. More density levels require an upstream component change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelDensity {
    Hidden,
    #[default]
    Shown,
}

/// Owner policy for the management surface. These defaults are initial values
/// shown by the shell, not private constants that silently override a choice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManagementSettings {
    pub stale_age_minutes: u32,
    pub announce_history_bound: usize,
    pub force_strength: f32,
    pub layout_damping: f32,
    pub label_density: LabelDensity,
    pub show_last_known: bool,
}

impl Default for ManagementSettings {
    fn default() -> Self {
        Self {
            stale_age_minutes: 15,
            announce_history_bound: 256,
            force_strength: 1.0,
            layout_damping: 2.5,
            label_density: LabelDensity::Shown,
            show_last_known: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NetworkRequest {
    Reconcile(NetworkInput),
    Pin(NodeKey, euclid::default::Point2D<f32>),
    Unpin(NodeKey),
}

pub const VOICE_ENCODING_OPTIONS: [&str; 3] =
    ["Pipit LPC-10", "Pipit LPC-10 half-rate", "Pipit IMA ADPCM"];
pub const VOICE_DURATION_OPTIONS: [&str; 3] = ["10 seconds", "30 seconds", "60 seconds"];
const VOICE_DURATION_SECONDS: [u32; 3] = [10, 30, 60];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoiceActivity {
    #[default]
    Idle,
    StartingCapture,
    Recording,
    StoppingCapture,
    StartingPlayback,
    Playing,
}

impl VoiceActivity {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::StartingCapture => "Requesting microphone",
            Self::Recording => "Recording",
            Self::StoppingCapture => "Finishing recording",
            Self::StartingPlayback => "Opening output",
            Self::Playing => "Playing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioRequest {
    StartCapture {
        device_id: String,
        max_duration: Duration,
    },
    StopCapture,
    Play {
        device_id: String,
        voice: DecodedVoice,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VoiceDraft {
    sender: MessagePeer,
    recipient: MessagePeer,
    authored_unix_ms: u64,
    nonce: [u8; 32],
    encoding: VoiceEncoding,
}

/// An externally documented carrier profile, selected by the owner instead of inferred from a
/// serial transport. Each variant is intentionally package-specific.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V4ProductProfile {
    MeshnologyN39V42,
}

pub const MESHNOLOGY_N39_NAME: &str = "Meshnology N39 WiFi LoRa 32 V4 kit";
pub const MESHNOLOGY_N39_DOCUMENTATION_URL: &str =
    "https://wiki.meshnology.com/N39/Meshnology%20N39/";

/// A side-effecting step the view asks for and the application loop performs.
///
/// Views never touch a serial port or start a thread: a handler records the
/// intent, and the loop that owns the hardware fulfils it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Re-survey the machine's ports.
    Rescan,
    /// Take the selected port into the flow.
    ConfirmDevice,
    /// Take an explicitly named T114 UF2 volume into the flow.
    ConfirmMountedT114,
    /// Take an owner-confirmed port already running the captured T114 DFU loader into the flow.
    ConfirmT114Dfu,
    /// Take the selected package into the flow (this is where a refusal comes
    /// from).
    ConfirmFirmware,
    /// The owner approved the reviewed plan.
    ApproveChanges,
    /// Hand the approved plan to the worker.
    BeginInstall,
}

/// How the survey went, so an empty list can say which kind of empty it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurveyState {
    /// No survey has run yet.
    #[default]
    Unasked,
    /// A survey ran; `devices` is what it found (possibly nothing).
    Surveyed,
}

pub struct DesktopState {
    pub appearance: crate::appearance::AppearanceState,
    pub section: DesktopSection,
    pub management_settings: ManagementSettings,
    pub device_mere: DeviceMere,
    pub network_epoch: u64,
    pub network_layout: Option<LayoutSnapshot>,
    pub network_pan: (f32, f32),
    pub network_zoom: f32,
    pub selected_relation: Option<ManagementRelationId>,
    pub pending_network: Option<NetworkRequest>,
    /// The node under an active pointer drag and its latest pinned position.
    /// Paint echoes this locally so the node tracks the cursor without waiting
    /// on the layout actor's round trip; the physics remain authoritative for
    /// every other body.
    network_drag: Option<(NodeKey, euclid::default::Point2D<f32>)>,
    /// Live-station presentation status: connected, or why the actor stopped.
    /// Facts still arrive only through `apply_management_material`.
    pub station_notice: Option<String>,
    pub availability: Vec<AvailabilityCapture>,
    pub selected_availability: Option<usize>,
    pub observation_load_path: cambium::TextInput,
    pub observation_export_path: cambium::TextInput,
    pub observation_device_association: cambium::TextInput,
    pub observation_collecting: bool,
    pub observation_durable: bool,
    pub observation_retention_entries: usize,
    pub observation_retention_bytes: usize,
    pub observation_retention_age_ms: u64,
    pub observation_notice: Option<String>,
    pending_observation: Option<ObservationRequest>,

    pub message_store: MessageStore,
    pub message_local: Option<MessagePeer>,
    pub message_recipient: cambium::TextInput,
    pub message_draft: cambium::TextInput,
    pub message_contact_name: cambium::TextInput,
    pub selected_message: Option<MessageId>,
    pub message_notice: Option<String>,
    next_message_nonce: u64,
    pub voice_inputs: Vec<AudioDeviceChoice>,
    pub voice_outputs: Vec<AudioDeviceChoice>,
    pub voice_input: cambium::SelectState,
    pub voice_output: cambium::SelectState,
    pub voice_encoding: cambium::SelectState,
    pub voice_duration: cambium::SelectState,
    pub voice_activity: VoiceActivity,
    pub voice_playback_receipt: Option<PlaybackReceipt>,
    pending_voice_draft: Option<VoiceDraft>,
    pending_audio: Option<AudioRequest>,

    /// The owner flow. The only thing that can move a page.
    pub installer: FirmwareInstaller,
    /// The verified package catalog, or why it could not be loaded.
    pub catalog: Option<FirmwareCatalog>,
    pub catalog_error: Option<String>,

    pub devices: Vec<DeviceCandidate>,
    pub survey: SurveyState,
    pub selected_device: Option<usize>,
    pub selected_package: Option<usize>,
    /// An owner declaration for a silent serial device. A discovered Retinue
    /// banner remains Linkboy evidence; this is only the escape hatch for a
    /// foreign application that cannot name itself.
    pub selected_board_family: Option<BoardFamily>,
    /// The board revision the owner types. A plan is refused without it, and
    /// that refusal is shown rather than hidden behind a disabled control.
    pub board_revision: cambium::TextInput,
    /// A narrowly named, externally documented source for a revision. This is distinct from a
    /// typed carrier marking so the approved plan says why either claim is allowed.
    pub v4_product_profile: Option<V4ProductProfile>,
    /// A mounted `HT-n5262` UF2 volume, entered explicitly because a drive
    /// letter is a transport location rather than an inferred board identity.
    pub t114_uf2_volume: cambium::TextInput,
    /// Where the GUI retains the mounted bootloader record for a later serial
    /// DFU recovery. This is required for a silent foreign T114 plan.
    pub t114_loader_record: cambium::TextInput,

    /// The current refusal, as separate visible lines. Cleared when the owner
    /// changes something that could resolve it.
    pub refusal: Vec<String>,
    /// The execution event log, oldest first.
    pub notes: Vec<String>,
    /// Transfer progress, `0.0..=1.0`, while one is running.
    pub progress: Option<f32>,
    /// The last Signalman-owned execution stage a recovery reported.
    pub recovery: Option<FirmwareInstallRecovery>,
    pub recovery_instructions: Option<String>,
    /// Set once the plan has been handed to the worker, so it is handed over
    /// once and the Install page can say it is running.
    pub install_running: bool,

    /// What the view asked the application loop to do.
    pub pending: Option<Request>,
}

impl DesktopState {
    /// A fresh flow with a catalog loaded from `index_path`. A catalog that
    /// will not verify is a visible state, not a panic: the first page says so
    /// and the flow simply cannot leave the firmware step.
    pub fn new(index_path: &std::path::Path) -> Self {
        let (catalog, catalog_error) = match FirmwareCatalog::load(index_path) {
            Ok(catalog) => (Some(catalog), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            appearance: crate::appearance::AppearanceState::default(),
            section: DesktopSection::Devices,
            management_settings: ManagementSettings::default(),
            device_mere: DeviceMere::new(),
            network_epoch: 0,
            network_layout: None,
            network_pan: (0.0, 0.0),
            network_zoom: 1.0,
            selected_relation: None,
            pending_network: None,
            network_drag: None,
            station_notice: None,
            availability: Vec::new(),
            selected_availability: None,
            observation_load_path: cambium::TextInput::default(),
            observation_export_path: cambium::TextInput::default(),
            observation_device_association: cambium::TextInput::default(),
            observation_collecting: false,
            observation_durable: false,
            observation_retention_entries: 4096,
            observation_retention_bytes: 512 * 1024,
            observation_retention_age_ms: 7 * 24 * 60 * 60 * 1000,
            observation_notice: None,
            pending_observation: None,
            message_store: MessageStore::memory("signalman-local"),
            message_local: None,
            message_recipient: cambium::TextInput::default(),
            message_draft: cambium::TextInput::default(),
            message_contact_name: cambium::TextInput::default(),
            selected_message: None,
            message_notice: None,
            next_message_nonce: 0,
            voice_inputs: Vec::new(),
            voice_outputs: Vec::new(),
            voice_input: cambium::SelectState::new(0).with_label("Voice input device"),
            voice_output: cambium::SelectState::new(0).with_label("Voice output device"),
            voice_encoding: cambium::SelectState::new(0).with_label("Voice encoding"),
            voice_duration: cambium::SelectState::new(1).with_label("Maximum recording duration"),
            voice_activity: VoiceActivity::Idle,
            voice_playback_receipt: None,
            pending_voice_draft: None,
            pending_audio: None,
            installer: FirmwareInstaller::new(),
            catalog,
            catalog_error,
            devices: Vec::new(),
            survey: SurveyState::default(),
            selected_device: None,
            selected_package: None,
            selected_board_family: None,
            board_revision: cambium::TextInput::default(),
            v4_product_profile: None,
            t114_uf2_volume: cambium::TextInput::default(),
            t114_loader_record: cambium::TextInput::default(),
            refusal: Vec::new(),
            notes: Vec::new(),
            progress: None,
            recovery: None,
            recovery_instructions: None,
            install_running: false,
            pending: None,
        }
    }

    pub fn show_section(&mut self, section: DesktopSection) {
        self.section = section;
    }
}

fn parse_address(text: &str) -> Option<[u8; 16]> {
    let text = text.trim();
    if text.len() != 32 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (slot, pair) in bytes
        .iter_mut()
        .zip(text.as_bytes().as_chunks::<2>().0.iter())
    {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        *slot = (high << 4) | low;
    }
    Some(bytes)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(port: &str) -> DeviceCandidate {
        DeviceCandidate {
            port: port.into(),
            board: None,
            banner: String::new(),
            region: None,
            channel: None,
            known: false,
        }
    }

    #[test]
    fn changing_selected_device_clears_its_observation_association() {
        let mut state = DesktopState::new(std::path::Path::new("missing-catalog.toml"));
        state.adopt_survey(vec![candidate("COM7"), candidate("COM10")]);
        state.select_device(0);
        state.observation_device_association = cambium::TextInput::new("v");

        state.select_device(1);

        assert!(state.observation_device_association.text().is_empty());
    }

    #[test]
    fn reselecting_the_same_device_keeps_its_observation_association() {
        let mut state = DesktopState::new(std::path::Path::new("missing-catalog.toml"));
        state.adopt_survey(vec![candidate("COM7")]);
        state.select_device(0);
        state.observation_device_association = cambium::TextInput::new("v4-usb-identity");

        state.select_device(0);

        assert_eq!(
            state.observation_device_association.text(),
            "v4-usb-identity"
        );
    }

    #[test]
    fn survey_that_loses_selected_device_clears_its_observation_association() {
        let mut state = DesktopState::new(std::path::Path::new("missing-catalog.toml"));
        state.adopt_survey(vec![candidate("COM7")]);
        state.select_device(0);
        state.observation_device_association = cambium::TextInput::new("v4-usb-identity");

        state.adopt_survey(vec![candidate("COM10")]);

        assert_eq!(state.selected_device, None);
        assert!(state.observation_device_association.text().is_empty());
    }
}
