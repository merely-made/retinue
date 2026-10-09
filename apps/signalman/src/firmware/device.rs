//! Device discovery and observation, plus owner-facing event text.

use std::path::Path;

use linkboy::{DeviceObservation, FlashEvent, FlowError};

use super::FirmwareError;

/// One device an owner can choose.
///
/// A port is a location, not an identity: the board is what the banner said. A silent port is
/// still listed, because a board that has stopped talking is the one an owner needs to recover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCandidate {
    /// The transport address (`COM7`, `/dev/ttyUSB0`).
    pub port: String,
    /// What the board said it is, or `None` for a silent port.
    pub board: Option<String>,
    /// The banner verbatim, for a person to read when the board is unknown.
    pub banner: String,
    pub region: Option<String>,
    pub channel: Option<String>,
    /// Whether this build can flash it.
    pub known: bool,
}

impl DeviceCandidate {
    /// The one-line description a chooser shows.
    pub fn summary(&self) -> String {
        match (&self.board, self.known) {
            (Some(board), true) => {
                let mut line = format!("{} — {board}", self.port);
                if let Some(region) = &self.region {
                    line.push_str(&format!(", region {region}"));
                }
                if let Some(channel) = &self.channel {
                    line.push_str(&format!(", channel {channel}"));
                }
                line
            }
            (Some(board), false) => {
                format!("{} — {board} (this build cannot flash it)", self.port)
            }
            (None, _) => format!("{} — silent (not running, or in use)", self.port),
        }
    }
}

/// Every serial port this machine has, asked what it is.
///
/// What a port *is* stays Linkboy's decision. No ports is an empty list, not an error.
pub fn survey_devices() -> Vec<DeviceCandidate> {
    let Ok(ports) = linkboy::ports() else {
        return Vec::new();
    };
    survey_ports(ports)
}

/// Ask only the named serial ports what they are, leaving every other serial device alone.
pub fn survey_ports(ports: impl IntoIterator<Item = String>) -> Vec<DeviceCandidate> {
    ports
        .into_iter()
        .map(|port| {
            let found = linkboy::identify(&port);
            let known = matches!(
                found.board,
                Some(linkboy::Board::HeltecV4 | linkboy::Board::T114)
            );
            DeviceCandidate {
                port: found.port,
                board: found.board.map(|board| match board {
                    linkboy::Board::Unknown(line) => line,
                    other => format!("{other:?}"),
                }),
                banner: found.banner,
                region: found.region,
                channel: found.channel,
                known,
            }
        })
        .collect()
}

/// The device observation for a chosen port, built as `linkboy plan` and `linkboy flash` do.
///
/// Includes the ESP ROM discovery pass, without which a V4 plan lacks processor and flash facts.
pub fn observe_device(
    port: &str,
    selection: Option<(linkboy::BoardFamily, String)>,
) -> Result<DeviceObservation, FirmwareError> {
    observe_device_with_t114_loader_snapshot(port, selection, None)
}

/// Observe a serial application port with an optional T114 loader record captured from that
/// same board's mounted UF2 interface. The record is required for a silent foreign T114:
/// serial DFU does not report the processor, capacity, or SoftDevice facts on its own.
pub fn observe_device_with_t114_loader_snapshot(
    port: &str,
    selection: Option<(linkboy::BoardFamily, String)>,
    loader_snapshot: Option<&linkboy::T114LoaderSnapshot>,
) -> Result<DeviceObservation, FirmwareError> {
    observe_device_with_board_selection_and_t114_loader_snapshot(
        port,
        selection
            .map(|(family, revision)| linkboy::BoardSelection::owner_confirmed(family, revision)),
        loader_snapshot,
    )
}

/// Observe a serial application port with a complete owner-confirmed board selection.
///
/// A documented product profile is accepted only as that named profile, never as permission to
/// treat every board in the same family as interchangeable.
pub fn observe_device_with_board_selection_and_t114_loader_snapshot(
    port: &str,
    selection: Option<linkboy::BoardSelection>,
    loader_snapshot: Option<&linkboy::T114LoaderSnapshot>,
) -> Result<DeviceObservation, FirmwareError> {
    let found = linkboy::identify(port);
    let mut observation = DeviceObservation::from_found(&found);
    if let Some(selection) = selection.clone() {
        observation = observation.confirm_board_selection(selection);
    }
    if let Some(snapshot) = loader_snapshot {
        if !matches!(
            selection.as_ref().map(|selection| &selection.family),
            Some(linkboy::BoardFamily::T114)
        ) {
            return Err(FirmwareError::LoaderSnapshot(
                "a T114 loader record requires an explicit t114@revision selection".into(),
            ));
        }
        let facts = snapshot.serial_dfu_observation();
        observation = observation.with_hardware(linkboy::HardwareFacts {
            processor: facts.processor.clone(),
            flash_size: facts.flash_size,
            bootloader: facts.bootloader.clone(),
            loader_route: Some("captured-t114-loader-snapshot".into()),
            bootloader_usb: Some(facts),
        });
    }
    if linkboy::needs_esp_rom_probe(&observation) {
        let mut process = linkboy::SystemProcessRunner::default();
        let facts = linkboy::route::esp_rom::discover(&mut process, port)
            .map_err(|error| FirmwareError::Execution(linkboy::ExecutionError::Process(error)))?;
        observation = observation.with_hardware(linkboy::HardwareFacts {
            processor: facts.processor.clone(),
            flash_size: facts.flash_size,
            bootloader: facts.bootloader.clone(),
            loader_route: Some("esp-rom".into()),
            bootloader_usb: Some(facts),
        });
    }
    Ok(observation)
}

/// Observe an owner-selected port that is already running the T114 serial-DFU loader.
///
/// The retained loader record supplies the hardware facts; the transport state tells Linkboy to
/// invoke the DFU helper directly rather than ask an absent application to enter the bootloader.
pub fn observe_t114_serial_dfu_port(
    port: &str,
    revision: String,
    loader_snapshot: &linkboy::T114LoaderSnapshot,
) -> DeviceObservation {
    let facts = loader_snapshot.serial_dfu_observation();
    DeviceObservation::from_bootloader(
        linkboy::DeviceTransport::SerialDfuPort(port.to_string()),
        facts.clone(),
    )
    .confirm_board(linkboy::BoardFamily::T114, revision)
    .with_hardware(linkboy::HardwareFacts {
        processor: facts.processor.clone(),
        flash_size: facts.flash_size,
        bootloader: facts.bootloader.clone(),
        loader_route: Some("captured-t114-loader-snapshot".into()),
        bootloader_usb: Some(facts),
    })
}

/// Observe an explicitly named mounted T114 UF2 volume, retaining the record needed for a
/// later serial-DFU restore. The owner still supplies the board revision; the mounted volume
/// proves the loader profile but not a revision printed on the carrier.
pub fn observe_t114_uf2_volume(
    volume: &str,
    revision: String,
) -> Result<(DeviceObservation, linkboy::T114LoaderSnapshot), FirmwareError> {
    let (observation, snapshot) =
        linkboy::t114_uf2_observation(volume).map_err(FirmwareError::Discovery)?;
    Ok((
        observation.confirm_board(linkboy::BoardFamily::T114, revision),
        snapshot,
    ))
}

/// Capture the mounted bootloader record at the owner-selected path, then return the immutable
/// observation for the UF2 package plan.
pub fn capture_t114_uf2_volume(
    volume: &str,
    revision: String,
    record_path: impl AsRef<Path>,
) -> Result<DeviceObservation, FirmwareError> {
    let (observation, snapshot) = observe_t114_uf2_volume(volume, revision)?;
    snapshot
        .save_json(record_path)
        .map_err(|error| FirmwareError::LoaderSnapshot(error.to_string()))?;
    Ok(observation)
}

/// A refusal, as separate visible lines. Every structured reason is rendered; none is
/// summarized away.
pub fn refusal_lines(error: &FlowError) -> Vec<String> {
    match error {
        FlowError::Refused(refusal) => refusal.reasons.iter().map(ToString::to_string).collect(),
        other => vec![other.to_string()],
    }
}

/// One owner-facing line for an execution event, or `None` for the terminal events a face shows
/// structurally as the receipt or the recovery page.
pub fn describe_event(event: &FlashEvent) -> Option<String> {
    Some(match event {
        FlashEvent::Inspecting { device, package_id } => {
            format!("Inspecting {device} for {package_id}")
        }
        FlashEvent::WaitingForOwnerAction { message } => message.clone(),
        FlashEvent::EnteringBootloader => "Putting the board in its bootloader".into(),
        FlashEvent::Rediscovering => "Waiting for the board to come back".into(),
        FlashEvent::Erasing => "Erasing".into(),
        FlashEvent::Writing { written, total } => {
            let pct = if *total > 0 {
                (*written as f64 / *total as f64 * 100.0).round() as u32
            } else {
                0
            };
            format!("Writing {written} of {total} bytes ({pct}%)")
        }
        FlashEvent::VerifyingTransfer => "Verifying what was written".into(),
        FlashEvent::Rebooting => "Rebooting the board".into(),
        FlashEvent::VerifyingApplication => "Asking the board what it is now".into(),
        FlashEvent::Complete { .. }
        | FlashEvent::ManualCheckRequired { .. }
        | FlashEvent::RecoveryRequired { .. } => return None,
        FlashEvent::Refused { reasons } => {
            format!(
                "Refused: {}",
                reasons
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        }
    })
}

/// How far through the write an event puts the transfer, as `0.0..=1.0`.
/// `None` when the event carries no progress.
pub fn event_progress(event: &FlashEvent) -> Option<f32> {
    match event {
        FlashEvent::Writing { written, total } if *total > 0 => {
            Some((*written as f32 / *total as f32).clamp(0.0, 1.0))
        }
        _ => None,
    }
}
