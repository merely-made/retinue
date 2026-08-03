//! The direct-PHY command loop, once, for every board.
//!
//! Both firmware images carried this identically apart from the transport underneath it and
//! the face above it. The transport is now [`HostLink`]; the radio and the face are now the
//! [`Executive`], which is also what keeps this from reaching the chip directly.
//!
//! What stays in a firmware binary: the executor, the select between host and radio, the
//! text probes (`status`, `sync`, `ui`, `bootloader`), and anything chip-specific. Chip
//! diagnostics are the clearest case — `sx126x_diagnostics` lives on the SX126x kind rather
//! than on `LoRa`, so a TX timeout is *reported* here and *acted on* by the board that knows
//! which chip it has.

use lora_phy::DelayNs;
use lora_phy::mod_traits::RadioKind;
use radio_face::{LedSignal, RxSummary, TxResult, WireError};
use selvage::{
    CONFIG_COMMAND_LEN, CommandEvent, CommandKind, CommandStream, EVENT_CONFIG, EVENT_RX, EVENT_TX,
    EVENT_UI_SNAPSHOT, MAX_UI_SNAPSHOT_LEN, TX_ACCEPTED, TX_TIMEOUT, TX_TOO_LONG,
    TX_UNKNOWN_COMMAND, UI_SNAPSHOT_ACCEPTED, UI_SNAPSHOT_MALFORMED, UI_SNAPSHOT_TOO_LONG,
    UI_SNAPSHOT_UNSUPPORTED_VERSION, decode_config_command, decode_ui_snapshot_command,
};

use crate::executive::{ChipDiagnostics, Executive};
use crate::link::{Flow, HostLink};
use crate::service;

/// What the caller must do after a batch of host bytes.
pub struct Outcome {
    pub flow: Flow,
}

/// Frame a received packet as an `EVENT_RX` and hand it to the host.
pub async fn on_radio_frame<L, RK, DLY>(
    link: &mut L,
    exec: &mut Executive<'_, RK, DLY>,
    frame: &[u8],
    rssi: i16,
    snr: i16,
) -> Flow
where
    L: HostLink,
    RK: RadioKind,
    DLY: DelayNs,
{
    let mut event = [0_u8; 7 + selvage::MAX_RADIO_FRAME_LEN];
    let length = frame.len().min(selvage::MAX_RADIO_FRAME_LEN);
    event[0] = EVENT_RX;
    event[1..3].copy_from_slice(&(length as u16).to_le_bytes());
    event[3..5].copy_from_slice(&rssi.to_le_bytes());
    event[5..7].copy_from_slice(&snr.to_le_bytes());
    event[7..7 + length].copy_from_slice(&frame[..length]);

    let flow = Flow::from(link.write_all(&event[..7 + length]).await);
    if flow == Flow::Detach {
        return flow;
    }

    let status = exec.status_mut();
    status.rx_frames = status.rx_frames.saturating_add(1);
    status.last_rx = Some(RxSummary {
        frame_len: length as u16,
        rssi_dbm: rssi,
        snr_tenths_db: snr.saturating_mul(10),
    });
    status.last_wake = radio_face::WakeSource::Radio;
    exec.publish(LedSignal::Activity);
    flow
}

/// Push host bytes through the command parser, acting on each complete command.
///
/// The caller owns the parser and its buffer across calls, because a command may span any
/// number of reads.
pub async fn on_host_bytes<L, RK, DLY, D>(
    link: &mut L,
    exec: &mut Executive<'_, RK, DLY>,
    stream: &mut CommandStream,
    command: &mut [u8; selvage::MAX_COMMAND_LEN],
    diagnostics: &D,
    bytes: &[u8],
) -> Outcome
where
    L: HostLink,
    RK: RadioKind,
    DLY: DelayNs,
    D: ChipDiagnostics<RK, DLY>,
{
    let mut outcome = Outcome {
        flow: Flow::Continue,
    };

    for &byte in bytes {
        let event = stream.push(byte, command);
        let flow = match event {
            CommandEvent::Pending => Flow::Continue,
            CommandEvent::Unknown { .. } => {
                Flow::from(link.write_all(&[EVENT_TX, TX_UNKNOWN_COMMAND, 0, 0]).await)
            }
            CommandEvent::TooLong {
                kind: CommandKind::UiSnapshot,
                ..
            } => Flow::from(
                link.write_all(&[EVENT_UI_SNAPSHOT, UI_SNAPSHOT_TOO_LONG])
                    .await,
            ),
            CommandEvent::TooLong {
                kind: CommandKind::Transmit,
                declared,
            } => {
                let length = (declared as u16).to_le_bytes();
                Flow::from(
                    link.write_all(&[EVENT_TX, TX_TOO_LONG, length[0], length[1]])
                        .await,
                )
            }
            CommandEvent::TooLong {
                kind: CommandKind::Configure,
                ..
            } => unreachable!("configure commands have a fixed length"),
            CommandEvent::Complete {
                kind: CommandKind::UiSnapshot,
                len,
            } => {
                let result = accept_snapshot(&command[..len], exec);
                Flow::from(link.write_all(&[EVENT_UI_SNAPSHOT, result]).await)
            }
            CommandEvent::Complete {
                kind: CommandKind::Configure,
                len,
            } => {
                debug_assert_eq!(len, CONFIG_COMMAND_LEN);
                let result = match decode_config_command(&command[..CONFIG_COMMAND_LEN]) {
                    Ok(profile) => exec.apply_profile(&profile).await,
                    Err(_) => service::MALFORMED,
                };
                Flow::from(link.write_all(&[EVENT_CONFIG, result]).await)
            }
            CommandEvent::Complete {
                kind: CommandKind::Transmit,
                len,
            } => {
                let frame_len = (len - 3) as u16;
                let result = exec.transmit(&command[3..len]).await;

                // A timed-out transmit leaves the radio in an unknown state, so the chip's
                // registers go out FIRST: the host attaches the most recent diagnostic to a
                // failed transmit, and one arriving after the reply would be lost from the
                // failure it explains.
                if result == TX_TIMEOUT {
                    let status = exec.status_mut();
                    status.radio = radio_face::RadioState::Fault;
                    status.fault = Some(radio_face::Fault {
                        code: 7,
                        message: radio_face::Text::from_truncated("TX TIMEOUT"),
                    });
                    exec.publish(LedSignal::Idle);
                    let body = exec.diagnostics(diagnostics).await;
                    if link.write_all(&body).await.is_err() {
                        outcome.flow = Flow::Detach;
                        return outcome;
                    }
                }

                let length = frame_len.to_le_bytes();
                let flow = Flow::from(
                    link.write_all(&[EVENT_TX, result, length[0], length[1]])
                        .await,
                );
                let status = exec.status_mut();
                if result == TX_ACCEPTED {
                    status.tx_frames = status.tx_frames.saturating_add(1);
                    status.last_tx = TxResult::Sent { frame_len };
                } else {
                    status.last_tx = TxResult::Failed { code: result };
                }
                exec.publish(LedSignal::Activity);
                flow
            }
        };

        if flow == Flow::Detach {
            outcome.flow = Flow::Detach;
            return outcome;
        }
    }

    outcome
}

/// Decode a host snapshot and hand it to the face, returning its result code.
fn accept_snapshot<RK: RadioKind, DLY: DelayNs>(body: &[u8], exec: &Executive<'_, RK, DLY>) -> u8 {
    let mut bytes = [0_u8; MAX_UI_SNAPSHOT_LEN];
    match decode_ui_snapshot_command(body, &mut bytes) {
        Ok(len) => match radio_face::decode_snapshot(&bytes[..len]) {
            Ok(snapshot) => {
                exec.publish_host(snapshot);
                UI_SNAPSHOT_ACCEPTED
            }
            Err(WireError::UnsupportedVersion(_)) => UI_SNAPSHOT_UNSUPPORTED_VERSION,
            Err(WireError::TooLong) => UI_SNAPSHOT_TOO_LONG,
            Err(_) => UI_SNAPSHOT_MALFORMED,
        },
        Err(selvage::UiSnapshotWireError::TooLong) => UI_SNAPSHOT_TOO_LONG,
        Err(_) => UI_SNAPSHOT_MALFORMED,
    }
}
