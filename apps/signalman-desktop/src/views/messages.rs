//! The Messages section: text, voice drops, and saved contacts.

use cambium::{button, el, text};
use signalman::message::{MessageDirection, MessageId};

use crate::state::{DesktopState, VOICE_DURATION_OPTIONS, VOICE_ENCODING_OPTIONS, VoiceActivity};

use super::{Child, heading};

pub(super) fn messages_page(state: &DesktopState) -> Child {
    let rows = state
        .message_store
        .records()
        .rev()
        .map(|record| {
            let id = record.message.id();
            let peer = match record.direction {
                MessageDirection::Incoming => record.message.sender(),
                MessageDirection::Outgoing => record.message.recipient(),
            };
            let name = state
                .message_store
                .contact_name(peer)
                .map(str::to_owned)
                .unwrap_or_else(|| short_address(peer.destination));
            let direction = match record.direction {
                MessageDirection::Incoming => "From",
                MessageDirection::Outgoing => "To",
            };
            let content = record.message.text().map(str::to_owned).unwrap_or_else(|| {
                let facts = record.message.voice().unwrap().facts();
                format!(
                    "Voice drop, {}, {} ms, {} bytes",
                    facts.encoding.label(),
                    facts.duration_ms,
                    facts.encoded_bytes
                )
            });
            let view = signalman::message_view::MessageView::new(record);
            let label = format!("{direction} {name}: {}. {}", content, view.delivery_text());
            Box::new(
                button(label, move |s: &mut DesktopState, _| s.select_message(id))
                    .attr(
                        "class",
                        if state.selected_message == Some(id) {
                            "message-row selected"
                        } else {
                            "message-row"
                        },
                    )
                    .attr("data-message-id", message_id_hex(id)),
            ) as Child
        })
        .collect::<Vec<_>>();

    let history: Child = if rows.is_empty() {
        Box::new(el("div", text("There are no persisted messages yet.")).attr("class", "empty"))
    } else {
        Box::new(
            el("div", rows)
                .attr("class", "message-rows")
                .attr("role", "list"),
        )
    };

    let contact_controls: Child = state
        .selected_message
        .and_then(|id| {
            state
                .message_store
                .records()
                .find(|record| record.message.id() == id)
        })
        .filter(|record| record.direction == MessageDirection::Incoming)
        .map(|record| record.message.sender())
        .filter(|peer| peer.identity.is_some() && state.message_store.contact_name(*peer).is_none())
        .map(|_| {
            Box::new(
                el(
                    "div",
                    (
                        el("div", text("Save this authenticated sender"))
                            .attr("class", "network-heading"),
                        el(
                            "label",
                            (
                                el("div", text("Your name for them")).attr("class", "field-label"),
                                el(
                                    "div",
                                    cambium::lens(
                                        |input: &mut cambium::TextInput| cambium::text_field(input),
                                        |s: &mut DesktopState| &mut s.message_contact_name,
                                    ),
                                )
                                .attr("class", "revision-wrap")
                                .attr("data-text-field", "message-contact-name"),
                            ),
                        )
                        .attr("class", "revision-label"),
                        button("Save sender as contact", |s: &mut DesktopState, _| {
                            s.save_selected_sender()
                        })
                        .attr("class", "secondary"),
                    ),
                )
                .attr("class", "message-contact"),
            ) as Child
        })
        .unwrap_or_else(|| Box::new(el("div", ()).attr("class", "empty-none")));

    let notice: Child = state
        .message_notice
        .as_ref()
        .map(|notice| {
            Box::new(
                el("div", text(notice.clone()))
                    .attr("class", "message-notice")
                    .attr("role", "status"),
            ) as Child
        })
        .unwrap_or_else(|| Box::new(el("div", ()).attr("class", "empty-none")));

    let input_names = state
        .voice_inputs
        .iter()
        .map(|device| {
            if device.is_default {
                format!("{} (system default)", device.label)
            } else {
                device.label.clone()
            }
        })
        .collect::<Vec<_>>();
    let input_control: Child = if input_names.is_empty() {
        Box::new(el("div", text("No voice input device is available.")).attr("class", "hint"))
    } else {
        Box::new(
            el(
                "label",
                (
                    el("div", text("Input device")).attr("class", "field-label"),
                    cambium::lens(
                        move |choice: &mut cambium::SelectState| {
                            let options =
                                input_names.iter().map(String::as_str).collect::<Vec<_>>();
                            cambium::select(choice, &options)
                        },
                        |s: &mut DesktopState| &mut s.voice_input,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
        )
    };

    let output_names = state
        .voice_outputs
        .iter()
        .map(|device| {
            if device.is_default {
                format!("{} (system default)", device.label)
            } else {
                device.label.clone()
            }
        })
        .collect::<Vec<_>>();
    let output_control: Child = if output_names.is_empty() {
        Box::new(el("div", text("No voice output device is available.")).attr("class", "hint"))
    } else {
        Box::new(
            el(
                "label",
                (
                    el("div", text("Output device")).attr("class", "field-label"),
                    cambium::lens(
                        move |choice: &mut cambium::SelectState| {
                            let options =
                                output_names.iter().map(String::as_str).collect::<Vec<_>>();
                            cambium::select(choice, &options)
                        },
                        |s: &mut DesktopState| &mut s.voice_output,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
        )
    };

    let capture_control: Child = match state.voice_activity {
        VoiceActivity::Idle => Box::new(
            button("Record voice drop", |s: &mut DesktopState, _| {
                s.start_voice_capture()
            })
            .attr("class", "primary")
            .attr("data-voice-action", "record"),
        ),
        VoiceActivity::Recording => Box::new(
            button("Stop and queue voice drop", |s: &mut DesktopState, _| {
                s.stop_voice_capture()
            })
            .attr("class", "primary")
            .attr("data-voice-action", "stop"),
        ),
        _ => Box::new(
            el(
                "div",
                text(format!("Host audio: {}.", state.voice_activity.label())),
            )
            .attr("class", "voice-activity")
            .attr("role", "status"),
        ),
    };

    let selected_is_voice = state.selected_message.is_some_and(|id| {
        state
            .message_store
            .records()
            .find(|record| record.message.id() == id)
            .is_some_and(|record| record.message.voice().is_some())
    });
    let playback_control: Child = if selected_is_voice {
        Box::new(
            button("Play selected voice drop", |s: &mut DesktopState, _| {
                s.play_selected_voice()
            })
            .attr("class", "secondary")
            .attr("data-voice-action", "play"),
        )
    } else {
        Box::new(
            el("div", text("Select a voice drop in history to play it.")).attr("class", "hint"),
        )
    };

    let playback_receipt: Child = state
        .voice_playback_receipt
        .as_ref()
        .map(|receipt| {
            Box::new(
                el(
                    "div",
                    text(format!(
                        "Playback receipt: {} ms, {} Hz, {} channel{} through {}.",
                        receipt.decoded_duration_ms,
                        receipt.output_sample_rate,
                        receipt.output_channels,
                        if receipt.output_channels == 1 {
                            ""
                        } else {
                            "s"
                        },
                        receipt.device_label,
                    )),
                )
                .attr("class", "voice-receipt"),
            ) as Child
        })
        .unwrap_or_else(|| Box::new(el("div", ()).attr("class", "empty-none")));

    let voice_controls = el(
        "div",
        (
            el("div", text("Voice drop")).attr("class", "network-heading"),
            el(
                "div",
                text("Recording is downmixed to 8 kHz mono, encoded once, and persisted before transport."),
            )
            .attr("class", "hint"),
            input_control,
            output_control,
            el(
                "label",
                (
                    el("div", text("Encoding")).attr("class", "field-label"),
                    cambium::lens(
                        |choice: &mut cambium::SelectState| {
                            cambium::select(choice, &VOICE_ENCODING_OPTIONS)
                        },
                        |s: &mut DesktopState| &mut s.voice_encoding,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
            el(
                "label",
                (
                    el("div", text("Maximum duration")).attr("class", "field-label"),
                    cambium::lens(
                        |choice: &mut cambium::SelectState| {
                            cambium::select(choice, &VOICE_DURATION_OPTIONS)
                        },
                        |s: &mut DesktopState| &mut s.voice_duration,
                    ),
                ),
            )
            .attr("class", "voice-choice"),
            capture_control,
            playback_control,
            playback_receipt,
        ),
    )
    .attr("class", "voice-compose");

    Box::new(
        el(
            "main",
            (
                heading(
                    "Messages",
                    "Conversation history is replayed from the local journal.",
                ),
                el(
                    "div",
                    (
                        el(
                            "label",
                            (
                                el("div", text("Recipient address"))
                                    .attr("class", "field-label"),
                                el(
                                    "div",
                                    cambium::lens(
                                        |input: &mut cambium::TextInput| cambium::text_field(input),
                                        |s: &mut DesktopState| &mut s.message_recipient,
                                    ),
                                )
                                .attr("class", "revision-wrap")
                                .attr("data-text-field", "message-recipient"),
                            ),
                        )
                        .attr("class", "revision-label"),
                        el(
                            "label",
                            (
                                el("div", text("Message"))
                                    .attr("class", "field-label"),
                                el(
                                    "div",
                                    cambium::lens(
                                        |input: &mut cambium::TextInput| cambium::text_field(input),
                                        |s: &mut DesktopState| &mut s.message_draft,
                                    ),
                                )
                                .attr("class", "revision-wrap")
                                .attr("data-text-field", "message-draft"),
                            ),
                        )
                        .attr("class", "revision-label"),
                        button("Queue message", |s: &mut DesktopState, _| s.queue_message())
                            .attr("class", "primary"),
                        el(
                            "div",
                            text(if state.message_local.is_some() {
                                "Outgoing intent is persisted before transport is attempted."
                            } else {
                                "A station identity is not connected. Drafts cannot be queued under an invented sender."
                            }),
                        )
                        .attr("class", "hint"),
                        notice,
                    ),
                )
                .attr("class", "message-compose"),
                voice_controls,
                el("div", text("Conversation history")).attr("class", "network-heading"),
                history,
                contact_controls,
            ),
        )
        .attr("class", "messages-page")
        .attr("role", "main")
        .attr("aria-label", "Messages"),
    )
}

fn short_address(bytes: [u8; 16]) -> String {
    bytes[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn message_id_hex(id: MessageId) -> String {
    id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}
