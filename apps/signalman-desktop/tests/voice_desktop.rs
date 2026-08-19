//! Deterministic desktop receipt for voice capture and playback state.
//!
//! Host device callbacks are covered by the CPAL boundary in `audio`; these
//! tests inject its typed events so CI never opens a microphone or speaker.

use std::time::Duration;

use cambium_genet_winit_host::Harness;
use genet_probe::Selector;
use signalman::message::{MessagePeer, MessageStatus};
use signalman::voice::VoiceEncoding;
use signalman_desktop::audio::{
    AudioDeviceChoice, AudioEvent, AudioInventory, CaptureStarted, CapturedVoice, PlaybackReceipt,
    PlaybackStarted,
};
use signalman_desktop::state::{
    AudioRequest, DesktopState, VOICE_DURATION_OPTIONS, VOICE_ENCODING_OPTIONS, VoiceActivity,
};
use signalman_desktop::views::{Child, Logic};
use signalman_desktop::{default_catalog_path, root, sheet};

type App = Harness<DesktopState, Logic, Child>;

fn choice(id: &str, label: &str, is_default: bool) -> AudioDeviceChoice {
    AudioDeviceChoice {
        id: id.into(),
        label: label.into(),
        is_default,
    }
}

fn state() -> DesktopState {
    let mut state = DesktopState::new(&default_catalog_path());
    state.adopt_audio_inventory(AudioInventory {
        inputs: vec![choice("wasapi:mic", "Bench microphone", true)],
        outputs: vec![choice("wasapi:speaker", "Bench speaker", true)],
    });
    state.set_message_local(MessagePeer::new([1; 16], Some([1; 32])));
    state.message_recipient = cambium::TextInput::new("02020202020202020202020202020202");
    state
}

fn captured() -> CapturedVoice {
    CapturedVoice {
        pcm: (0..8_000)
            .map(|sample| if sample % 80 < 40 { 5_000 } else { -5_000 })
            .collect(),
        device_id: "wasapi:mic".into(),
        device_label: "Bench microphone".into(),
        source_sample_rate: 48_000,
        source_channels: 2,
        captured_duration_ms: 1_000,
    }
}

#[test]
fn captured_pcm_is_encoded_once_persisted_and_then_played_from_the_selected_output() {
    let mut state = state();
    state.voice_encoding.selected = 1;
    state.voice_duration.selected = 0;
    assert_eq!(VOICE_ENCODING_OPTIONS[1], "Pipit LPC-10 half-rate");
    assert_eq!(VOICE_DURATION_OPTIONS[0], "10 seconds");

    state.start_voice_capture_at(1_000);
    assert_eq!(state.voice_activity, VoiceActivity::StartingCapture);
    assert_eq!(
        state.take_audio_request(),
        Some(AudioRequest::StartCapture {
            device_id: "wasapi:mic".into(),
            max_duration: Duration::from_secs(10),
        })
    );

    state.apply_audio_event_at(
        AudioEvent::CaptureStarted(CaptureStarted {
            device_id: "wasapi:mic".into(),
            device_label: "Bench microphone".into(),
            source_sample_rate: 48_000,
            source_channels: 2,
            max_duration_ms: 10_000,
        }),
        1_001,
    );
    assert_eq!(state.voice_activity, VoiceActivity::Recording);
    state.stop_voice_capture();
    assert_eq!(state.voice_activity, VoiceActivity::StoppingCapture);
    assert_eq!(state.take_audio_request(), Some(AudioRequest::StopCapture));

    state.apply_audio_event_at(AudioEvent::Captured(captured()), 2_000);
    assert_eq!(state.voice_activity, VoiceActivity::Idle);
    assert_eq!(state.message_store.len(), 1);
    assert_eq!(state.message_store.log_len(), 1);
    let record = state.message_store.records().next().unwrap();
    let voice = record.message.voice().unwrap();
    assert_eq!(voice.facts().encoding, VoiceEncoding::Lpc10Half);
    assert_eq!(voice.facts().sample_rate, 8_000);
    assert_eq!(voice.facts().duration_ms, 1_000);
    assert_eq!(
        record.status,
        MessageStatus::Queued(signalman::message::QueuedReason::Offline)
    );

    state.play_selected_voice();
    let request = state.take_audio_request().expect("playback request");
    let AudioRequest::Play { device_id, voice } = request else {
        panic!("expected playback");
    };
    assert_eq!(device_id, "wasapi:speaker");
    assert_eq!(voice.sample_rate, 8_000);
    assert_eq!(voice.decoded_duration_ms, 1_000);
    assert_eq!(voice.pcm.len(), 8_000);

    state.apply_audio_event_at(
        AudioEvent::PlaybackStarted(PlaybackStarted {
            device_id: "wasapi:speaker".into(),
            device_label: "Bench speaker".into(),
            output_sample_rate: 48_000,
            output_channels: 2,
            decoded_duration_ms: 1_000,
        }),
        2_001,
    );
    assert_eq!(state.voice_activity, VoiceActivity::Playing);
    let receipt = PlaybackReceipt {
        device_id: "wasapi:speaker".into(),
        device_label: "Bench speaker".into(),
        output_sample_rate: 48_000,
        output_channels: 2,
        decoded_duration_ms: 1_000,
    };
    state.apply_audio_event_at(AudioEvent::PlaybackFinished(receipt.clone()), 3_001);
    assert_eq!(state.voice_activity, VoiceActivity::Idle);
    assert_eq!(state.voice_playback_receipt, Some(receipt));
}

#[test]
fn capture_failure_never_creates_or_delivers_a_message() {
    let mut state = state();
    state.start_voice_capture_at(1_000);
    let _ = state.take_audio_request();
    state.apply_audio_event_at(
        AudioEvent::Failed {
            operation: signalman_desktop::audio::AudioOperation::Capture,
            message: "permission denied".into(),
        },
        1_001,
    );

    assert_eq!(state.voice_activity, VoiceActivity::Idle);
    assert_eq!(state.message_store.len(), 0);
    assert!(
        state
            .message_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("permission denied"))
    );
}

#[test]
fn shipping_messages_face_exposes_record_stop_and_play_actions_with_device_choices() {
    let mut harness: App = Harness::new(sheet(), state(), root as Logic);
    harness.layout_at(1_100.0, 900.0);
    assert!(harness.click_on(&Selector::role("button").containing("Messages")));
    harness.with_surfaces(|surfaces| {
        assert!(genet_probe::text_present(surfaces, "Bench microphone"));
        assert!(genet_probe::text_present(surfaces, "Bench speaker"));
        assert!(genet_probe::text_present(surfaces, "Pipit LPC-10"));
        assert!(genet_probe::text_present(surfaces, "30 seconds"));
    });

    assert!(harness.click_on(&Selector::role("button").containing("Record voice drop")));
    assert_eq!(
        harness.state().voice_activity,
        VoiceActivity::StartingCapture
    );
    harness.update(|state| {
        state.apply_audio_event_at(
            AudioEvent::CaptureStarted(CaptureStarted {
                device_id: "wasapi:mic".into(),
                device_label: "Bench microphone".into(),
                source_sample_rate: 48_000,
                source_channels: 2,
                max_duration_ms: 30_000,
            }),
            1_001,
        );
    });
    assert!(harness.click_on(&Selector::role("button").containing("Stop and queue voice drop")));
    let mut stopped = None;
    harness.update(|state| stopped = state.take_audio_request());
    assert_eq!(stopped, Some(AudioRequest::StopCapture));

    harness.update(|state| state.apply_audio_event_at(AudioEvent::Captured(captured()), 2_000));
    harness.with_surfaces(|surfaces| {
        assert!(genet_probe::text_present(surfaces, "Voice drop"));
        assert!(genet_probe::text_present(surfaces, "offline, queued"));
    });
    assert!(harness.click_on(&Selector::role("button").containing("Play selected voice drop")));
    let mut playback = None;
    harness.update(|state| playback = state.take_audio_request());
    assert!(matches!(playback, Some(AudioRequest::Play { .. })));
}
