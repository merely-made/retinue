//! Voice drops: host audio inventory, capture, and playback.

use std::time::Duration;

use signalman::message::{MessageEvent, MessagePeer, QueuedReason, VoiceMessage};
use signalman::voice::{VoiceClip, VoiceEncoding};

use crate::audio::{AudioEvent, AudioInventory, AudioOperation, CapturedVoice};

use super::{
    AudioRequest, DesktopState, VOICE_DURATION_SECONDS, VoiceActivity, VoiceDraft, parse_address,
    unix_ms,
};

impl DesktopState {
    pub fn adopt_audio_inventory(&mut self, inventory: AudioInventory) {
        let selected_input = self
            .voice_inputs
            .get(self.voice_input.selected)
            .map(|device| device.id.as_str());
        let selected_output = self
            .voice_outputs
            .get(self.voice_output.selected)
            .map(|device| device.id.as_str());
        let input_index = selected_input
            .and_then(|id| inventory.inputs.iter().position(|device| device.id == id))
            .or_else(|| inventory.inputs.iter().position(|device| device.is_default))
            .unwrap_or(0);
        let output_index = selected_output
            .and_then(|id| inventory.outputs.iter().position(|device| device.id == id))
            .or_else(|| {
                inventory
                    .outputs
                    .iter()
                    .position(|device| device.is_default)
            })
            .unwrap_or(0);
        self.voice_inputs = inventory.inputs;
        self.voice_outputs = inventory.outputs;
        self.voice_input.selected = input_index;
        self.voice_input.open = false;
        self.voice_output.selected = output_index;
        self.voice_output.open = false;
    }

    pub fn start_voice_capture(&mut self) {
        let now = unix_ms();
        self.start_voice_capture_at(now);
    }

    pub fn start_voice_capture_at(&mut self, observed_unix_ms: u64) {
        if self.voice_activity != VoiceActivity::Idle {
            self.message_notice = Some(format!(
                "Host audio is already {}.",
                self.voice_activity.label().to_ascii_lowercase()
            ));
            return;
        }
        let Some(sender) = self.message_local else {
            self.message_notice =
                Some("Connect a station identity before recording a voice drop.".into());
            return;
        };
        let Some(destination) = parse_address(self.message_recipient.text()) else {
            self.message_notice = Some("A recipient is exactly 32 hexadecimal characters.".into());
            return;
        };
        let Some(device) = self.voice_inputs.get(self.voice_input.selected) else {
            self.message_notice = Some("Select an available voice input device.".into());
            return;
        };
        let mut nonce = [0_u8; 32];
        nonce[..8].copy_from_slice(&self.next_message_nonce.to_be_bytes());
        nonce[8..16].copy_from_slice(&observed_unix_ms.to_be_bytes());
        self.pending_voice_draft = Some(VoiceDraft {
            sender,
            recipient: MessagePeer::new(destination, None),
            authored_unix_ms: observed_unix_ms,
            nonce,
            encoding: self.selected_voice_encoding(),
        });
        self.pending_audio = Some(AudioRequest::StartCapture {
            device_id: device.id.clone(),
            max_duration: Duration::from_secs(u64::from(self.selected_voice_duration_seconds())),
        });
        self.voice_activity = VoiceActivity::StartingCapture;
        self.voice_playback_receipt = None;
        self.message_notice = Some("Requesting access to the selected microphone.".into());
    }

    pub fn stop_voice_capture(&mut self) {
        if self.voice_activity != VoiceActivity::Recording {
            self.message_notice = Some("There is no active voice recording to finish.".into());
            return;
        }
        self.voice_activity = VoiceActivity::StoppingCapture;
        self.pending_audio = Some(AudioRequest::StopCapture);
        self.message_notice = Some("Finishing and encoding the voice recording.".into());
    }

    pub fn play_selected_voice(&mut self) {
        if self.voice_activity != VoiceActivity::Idle {
            self.message_notice = Some(format!(
                "Host audio is already {}.",
                self.voice_activity.label().to_ascii_lowercase()
            ));
            return;
        }
        let Some(device) = self.voice_outputs.get(self.voice_output.selected) else {
            self.message_notice = Some("Select an available voice output device.".into());
            return;
        };
        let Some(id) = self.selected_message else {
            self.message_notice = Some("Select a voice drop to play.".into());
            return;
        };
        let Some(voice) = self
            .message_store
            .records()
            .find(|record| record.message.id() == id)
            .and_then(|record| record.message.voice())
        else {
            self.message_notice = Some("The selected message is not a voice drop.".into());
            return;
        };
        let decoded = match voice.clip.decode() {
            Ok(decoded) => decoded,
            Err(error) => {
                self.message_notice = Some(format!("Voice drop could not be decoded: {error}"));
                return;
            }
        };
        self.pending_audio = Some(AudioRequest::Play {
            device_id: device.id.clone(),
            voice: decoded,
        });
        self.voice_activity = VoiceActivity::StartingPlayback;
        self.voice_playback_receipt = None;
        self.message_notice = Some("Opening the selected voice output.".into());
    }

    pub fn take_audio_request(&mut self) -> Option<AudioRequest> {
        self.pending_audio.take()
    }

    pub fn apply_audio_event(&mut self, event: AudioEvent) {
        self.apply_audio_event_at(event, unix_ms());
    }

    pub fn apply_audio_event_at(&mut self, event: AudioEvent, observed_unix_ms: u64) {
        match event {
            AudioEvent::CaptureStarted(started) => {
                self.voice_activity = VoiceActivity::Recording;
                self.message_notice = Some(format!(
                    "Recording from {} at {} Hz, {} channel{}. Maximum {} ms.",
                    started.device_label,
                    started.source_sample_rate,
                    started.source_channels,
                    if started.source_channels == 1 {
                        ""
                    } else {
                        "s"
                    },
                    started.max_duration_ms,
                ));
            }
            AudioEvent::Captured(captured) => {
                self.finish_captured_voice(captured, observed_unix_ms)
            }
            AudioEvent::PlaybackStarted(started) => {
                self.voice_activity = VoiceActivity::Playing;
                self.message_notice = Some(format!(
                    "Playing {} ms through {} at {} Hz, {} channel{}.",
                    started.decoded_duration_ms,
                    started.device_label,
                    started.output_sample_rate,
                    started.output_channels,
                    if started.output_channels == 1 {
                        ""
                    } else {
                        "s"
                    },
                ));
            }
            AudioEvent::PlaybackFinished(receipt) => {
                self.voice_activity = VoiceActivity::Idle;
                self.message_notice = Some(format!(
                    "Played {} ms through {}.",
                    receipt.decoded_duration_ms, receipt.device_label
                ));
                self.voice_playback_receipt = Some(receipt);
            }
            AudioEvent::Failed { operation, message } => {
                self.voice_activity = VoiceActivity::Idle;
                if operation == AudioOperation::Capture {
                    self.pending_voice_draft = None;
                }
                self.message_notice =
                    Some(format!("Voice {} failed: {message}", operation.label()));
            }
        }
    }

    fn finish_captured_voice(&mut self, captured: CapturedVoice, observed_unix_ms: u64) {
        self.voice_activity = VoiceActivity::Idle;
        let Some(draft) = self.pending_voice_draft.take() else {
            self.message_notice = Some("Captured audio had no pending voice draft.".into());
            return;
        };
        let clip = match VoiceClip::encode_pcm(&captured.pcm, draft.encoding) {
            Ok(clip) => clip,
            Err(error) => {
                self.message_notice =
                    Some(format!("Voice recording could not be encoded: {error}"));
                return;
            }
        };
        let message = match VoiceMessage::compose(
            draft.sender,
            draft.recipient,
            draft.authored_unix_ms,
            draft.nonce,
            clip,
        ) {
            Ok(message) => message,
            Err(error) => {
                self.message_notice = Some(format!("Voice recording could not be queued: {error}"));
                return;
            }
        };
        let id = message.id;
        match self.message_store.append(MessageEvent::OutgoingQueued {
            message: message.into(),
            reason: QueuedReason::Offline,
            observed_unix_ms,
        }) {
            Ok(_) => {
                self.next_message_nonce = self.next_message_nonce.saturating_add(1);
                self.selected_message = Some(id);
                self.message_notice = Some(format!(
                    "Voice drop persisted offline and queued: {} ms captured from {} at {} Hz, {} channel{}.",
                    captured.captured_duration_ms,
                    captured.device_label,
                    captured.source_sample_rate,
                    captured.source_channels,
                    if captured.source_channels == 1 {
                        ""
                    } else {
                        "s"
                    },
                ));
            }
            Err(error) => {
                self.message_notice = Some(format!("Voice drop was not queued: {error}"));
            }
        }
    }

    pub fn selected_voice_encoding(&self) -> VoiceEncoding {
        match self.voice_encoding.selected {
            1 => VoiceEncoding::Lpc10Half,
            2 => VoiceEncoding::ImaAdpcm,
            _ => VoiceEncoding::Lpc10,
        }
    }

    pub fn selected_voice_duration_seconds(&self) -> u32 {
        VOICE_DURATION_SECONDS
            .get(self.voice_duration.selected)
            .copied()
            .unwrap_or(30)
    }
}
