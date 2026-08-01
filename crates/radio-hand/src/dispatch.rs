//! The direct-PHY command loop, once, for every board.
//!
//! Both firmware images carried this identically apart from the transport underneath it and
//! the face above it. The transport is now [`HostLink`]; the face is a pair of function
//! pointers, because both boards already expose the same two free functions.
//!
//! What stays in a firmware binary: the executor, the select between host and radio, the
//! text probes (`status`, `sync`, `ui`, `bootloader`), and anything chip-specific. Chip
//! diagnostics are the clearest case — `sx126x_diagnostics` lives on the SX126x kind rather
//! than on `LoRa`, so a TX timeout is *reported* here through [`Outcome::tx_timed_out`] and
//! *acted on* by the board that knows which chip it has.

use embassy_time::{Duration, with_timeout};
use lora_phy::mod_params::{ModulationParams, PacketParams};
use lora_phy::mod_traits::RadioKind;
use lora_phy::{DelayNs, LoRa};
use radio_face::{HostSnapshot, LedSignal, LocalStatus, RxSummary, TxResult, WireError};
use selvage::{
    CONFIG_COMMAND_LEN, CommandEvent, CommandKind, CommandStream, EVENT_CONFIG, EVENT_RX, EVENT_TX,
    EVENT_UI_SNAPSHOT, MAX_UI_SNAPSHOT_LEN, TX_ACCEPTED, TX_RADIO_FAULT, TX_TIMEOUT, TX_TOO_LONG,
    TX_UNKNOWN_COMMAND, UI_SNAPSHOT_ACCEPTED, UI_SNAPSHOT_MALFORMED, UI_SNAPSHOT_TOO_LONG,
    UI_SNAPSHOT_UNSUPPORTED_VERSION, decode_config_command, decode_ui_snapshot_command,
};

use crate::link::{HostLink, LinkFault};
use crate::service;

/// How long a transmission may run before the firmware gives up on it.
const TX_DEADLINE: Duration = Duration::from_secs(3);

/// Whether the host session survives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep serving this peer.
    Continue,
    /// The peer is gone; abandon the session and wait for the next.
    Detach,
}

impl Flow {
    fn from(result: Result<(), LinkFault>) -> Self {
        match result {
            Ok(()) => Flow::Continue,
            Err(LinkFault::Detached) => Flow::Detach,
        }
    }
}

/// The radio settings a profile owns, held by the caller so a rejected profile cannot
/// half-apply: [`crate::service::apply_profile`] builds a replacement and only a complete
/// one is swapped in.
pub struct RadioState {
    pub modulation: ModulationParams,
    pub tx: PacketParams,
    pub rx: PacketParams,
    pub tx_power_dbm: i32,
    /// Set when the radio must be put back into receive before the next wait.
    pub prepare_rx: bool,
}

/// The board's local face. Function pointers rather than a trait: both images already
/// expose exactly these two signatures as free functions, so this costs nothing and needs
/// no borrow of the board's UI state.
pub struct Face {
    pub publish: fn(LocalStatus, LedSignal),
    pub publish_host: fn(HostSnapshot),
}

/// A board's ability to read its radio chip's diagnostic registers.
///
/// Chip-specific: `sx126x_diagnostics` lives on the SX126x kind, not on `LoRa`, so this
/// cannot be called generically. It takes the dispatch's own `lora` borrow rather than
/// holding one, which is what lets the shared loop emit a diagnostic mid-command.
///
/// The ordering it exists to preserve is load-bearing. The host takes the most recent
/// diagnostic when a transmit *fails* and attaches it to that failure, so a diagnostic
/// emitted after its `EVENT_TX` reply would be lost from the error it explains and
/// misattributed to the next one.
/// Auto traits are deliberately unbounded: these futures are polled by a
/// single-threaded embassy executor on the board, never sent across threads, so a
/// `Send` bound would buy nothing and would exclude the HAL types that implement this.
#[allow(async_fn_in_trait)]
pub trait ChipDiagnostics<RK: RadioKind, DLY: DelayNs> {
    /// The seven-byte `EVENT_DIAGNOSTIC` body, including its marker.
    async fn read(&self, lora: &mut LoRa<RK, DLY>) -> [u8; 7];
}

/// What the caller must do after a batch of host bytes.
pub struct Outcome {
    pub flow: Flow,
}

/// Frame a received packet as an `EVENT_RX` and hand it to the host.
pub async fn on_radio_frame<L: HostLink>(
    link: &mut L,
    status: &mut LocalStatus,
    face: &Face,
    frame: &[u8],
    rssi: i16,
    snr: i16,
) -> Flow {
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

    status.rx_frames = status.rx_frames.saturating_add(1);
    status.last_rx = Some(RxSummary {
        frame_len: length as u16,
        rssi_dbm: rssi,
        snr_tenths_db: snr.saturating_mul(10),
    });
    status.last_wake = radio_face::WakeSource::Radio;
    (face.publish)(*status, LedSignal::Activity);
    flow
}

/// Push host bytes through the command parser, acting on each complete command.
///
/// The caller owns the parser and its buffer across calls, because a command may span any
/// number of reads.
#[allow(clippy::too_many_arguments)]
pub async fn on_host_bytes<L, RK, DLY, D>(
    link: &mut L,
    lora: &mut LoRa<RK, DLY>,
    stream: &mut CommandStream,
    command: &mut [u8; selvage::MAX_COMMAND_LEN],
    radio: &mut RadioState,
    status: &mut LocalStatus,
    face: &Face,
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
                let result = accept_snapshot(&command[..len], face);
                Flow::from(link.write_all(&[EVENT_UI_SNAPSHOT, result]).await)
            }
            CommandEvent::Complete {
                kind: CommandKind::Configure,
                len,
            } => {
                debug_assert_eq!(len, CONFIG_COMMAND_LEN);
                let result = match decode_config_command(&command[..CONFIG_COMMAND_LEN]) {
                    Ok(profile) => match service::apply_profile(lora, &profile).await {
                        Ok(applied) => {
                            radio.modulation = applied.modulation;
                            radio.tx = applied.tx;
                            radio.rx = applied.rx;
                            radio.tx_power_dbm = applied.tx_power_dbm;
                            radio.prepare_rx = true;
                            crate::board_status::apply_profile(status, profile);
                            (face.publish)(*status, LedSignal::Idle);
                            service::ACCEPTED
                        }
                        Err(code) => code,
                    },
                    Err(_) => service::MALFORMED,
                };
                Flow::from(link.write_all(&[EVENT_CONFIG, result]).await)
            }
            CommandEvent::Complete {
                kind: CommandKind::Transmit,
                len,
            } => {
                let frame_len = (len - 3) as u16;
                let result = transmit(lora, radio, &command[3..len]).await;
                radio.prepare_rx = true;

                // A timed-out transmit leaves the radio in an unknown state, so the chip's
                // registers go out FIRST: the host attaches the most recent diagnostic to a
                // failed transmit, and one arriving after the reply would be lost from the
                // failure it explains.
                if result == TX_TIMEOUT {
                    status.radio = radio_face::RadioState::Fault;
                    status.fault = Some(radio_face::Fault {
                        code: 7,
                        message: radio_face::Text::from_truncated("TX TIMEOUT"),
                    });
                    (face.publish)(*status, LedSignal::Idle);
                    let body = diagnostics.read(lora).await;
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
                if result == TX_ACCEPTED {
                    status.tx_frames = status.tx_frames.saturating_add(1);
                    status.last_tx = TxResult::Sent { frame_len };
                } else {
                    status.last_tx = TxResult::Failed { code: result };
                }
                (face.publish)(*status, LedSignal::Activity);
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

/// Put one frame on the air, returning its `EVENT_TX` result code.
async fn transmit<RK, DLY>(lora: &mut LoRa<RK, DLY>, radio: &mut RadioState, frame: &[u8]) -> u8
where
    RK: RadioKind,
    DLY: DelayNs,
{
    let prepared = lora
        .prepare_for_tx(&radio.modulation, &mut radio.tx, radio.tx_power_dbm, frame)
        .await
        .is_ok();
    if !prepared {
        return TX_RADIO_FAULT;
    }
    match with_timeout(TX_DEADLINE, lora.tx()).await {
        Ok(Ok(())) => TX_ACCEPTED,
        Ok(Err(_)) => TX_RADIO_FAULT,
        Err(_) => TX_TIMEOUT,
    }
}

/// Decode a host snapshot and hand it to the face, returning its result code.
fn accept_snapshot(body: &[u8], face: &Face) -> u8 {
    let mut bytes = [0_u8; MAX_UI_SNAPSHOT_LEN];
    match decode_ui_snapshot_command(body, &mut bytes) {
        Ok(len) => match radio_face::decode_snapshot(&bytes[..len]) {
            Ok(snapshot) => {
                (face.publish_host)(snapshot);
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
