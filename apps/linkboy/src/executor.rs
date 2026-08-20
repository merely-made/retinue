//! Structured execution beneath CLI and graphical faces.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::device::DeviceTransport;
use crate::package::{
    ExpectedApplication, FirmwarePartKind, FlashPackage, FlashRoute, HelperRequirement,
    PayloadFormat, RecoveryInstructions, VerifiedPackagePart,
};
use crate::plan::{FlashPlan, RefusalReason};
use crate::receipt::{ApplicationVerification, FlashReceipt, ReceiptStage};
use crate::route::{adafruit_dfu, esp_rom};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessProgress {
    pub written: u64,
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessOutput {
    pub diagnostics: String,
}

pub trait ProcessRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        progress: &mut dyn FnMut(ProcessProgress),
    ) -> Result<ProcessOutput, ProcessFailure>;

    /// Validate a package-pinned helper before any destructive route stage. Test runners may
    /// leave the default in place because helper availability is injected separately there.
    fn verify_helper(&mut self, _requirement: &HelperRequirement) -> Result<(), ProcessFailure> {
        Ok(())
    }
}

#[derive(Default)]
pub struct SystemProcessRunner {
    verified_helpers: BTreeMap<String, PathBuf>,
}

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        progress: &mut dyn FnMut(ProcessProgress),
    ) -> Result<ProcessOutput, ProcessFailure> {
        let executable = self
            .verified_helpers
            .get(program)
            .cloned()
            // A non-writing loader probe happens before package planning, so it
            // cannot yet have a manifest requirement to verify. It must still
            // use the same installed helper location as the later write rather
            // than silently falling back to PATH.
            .unwrap_or(crate::helper::resolve_program(program)?);
        let output = Command::new(executable)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ProcessFailure::MissingHelper {
                        program: program.into(),
                    }
                } else {
                    ProcessFailure::Failed {
                        program: program.into(),
                        diagnostics: error.to_string(),
                    }
                }
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let diagnostics = format!("{stdout}{stderr}");
        for line in diagnostics.lines() {
            if let Some(progress_value) = generic_progress(line) {
                progress(progress_value);
            }
        }
        if !output.status.success() {
            return Err(ProcessFailure::Failed {
                program: program.into(),
                diagnostics: diagnostics.trim().to_string(),
            });
        }
        Ok(ProcessOutput { diagnostics })
    }

    fn verify_helper(&mut self, requirement: &HelperRequirement) -> Result<(), ProcessFailure> {
        let executable = crate::helper::resolve_program(&requirement.program)?;
        crate::helper::verify_file_digest(&executable, requirement)?;
        let executable_text = executable.to_string_lossy().into_owned();
        crate::helper::verify_installed_at(self, requirement, &executable_text)?;
        self.verified_helpers
            .insert(requirement.program.clone(), executable);
        Ok(())
    }
}

fn generic_progress(line: &str) -> Option<ProcessProgress> {
    crate::route::parse_progress_line(line)
}

pub trait DeviceRunner {
    fn enter_bootloader(
        &mut self,
        current_port: &str,
        patience: Duration,
    ) -> Result<String, DeviceFailure>;

    fn rediscover_application(
        &mut self,
        original_port: &str,
        bootloader_port: &str,
        expected: &ExpectedApplication,
        patience: Duration,
    ) -> Result<String, DeviceFailure>;

    fn verify_application(
        &mut self,
        application_port: &str,
        expected: &ExpectedApplication,
    ) -> Result<ApplicationVerification, DeviceFailure>;
}

#[derive(Default)]
pub struct LiveDeviceRunner;

impl DeviceRunner for LiveDeviceRunner {
    fn enter_bootloader(
        &mut self,
        current_port: &str,
        patience: Duration,
    ) -> Result<String, DeviceFailure> {
        crate::enter_bootloader(current_port, patience).map_err(DeviceFailure::from)
    }

    fn rediscover_application(
        &mut self,
        original_port: &str,
        bootloader_port: &str,
        expected: &ExpectedApplication,
        patience: Duration,
    ) -> Result<String, DeviceFailure> {
        let deadline = std::time::Instant::now() + patience;

        // A serial port can enumerate before the application behind it is ready to answer.
        // Probing during that window asserts DTR against a half-started T114 and can strand
        // its first CDC session before the board reaches the host loop. Give the verified
        // application image one bounded startup window before opening any returned port.
        std::thread::sleep(APPLICATION_STARTUP_GRACE.min(patience));
        while std::time::Instant::now() < deadline {
            let Ok(ports) = crate::ports() else {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            };

            if let Some(application_port) = select_application_port(
                &ports,
                original_port,
                bootloader_port,
                expected,
                identifies_expected_family,
            )? {
                return Ok(application_port);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(DeviceFailure::ApplicationTimeout(patience))
    }

    fn verify_application(
        &mut self,
        application_port: &str,
        expected: &ExpectedApplication,
    ) -> Result<ApplicationVerification, DeviceFailure> {
        let found = crate::identify(application_port);
        let Some(board) = found.board else {
            return Err(DeviceFailure::Silence(application_port.into()));
        };
        let Some(family) = crate::package::BoardFamily::from_board(&board) else {
            return Err(DeviceFailure::UnexpectedApplication {
                detail: format!("unknown board banner: {}", found.banner.trim()),
            });
        };
        if family != expected.board {
            return Err(DeviceFailure::UnexpectedApplication {
                detail: format!(
                    "expected {expected_board}, found {family}",
                    expected_board = expected.board
                ),
            });
        }
        let version = crate::field(&found.banner, "version=").ok_or_else(|| {
            DeviceFailure::UnexpectedApplication {
                detail: "application did not report version=".into(),
            }
        })?;
        if version != expected.version {
            return Err(DeviceFailure::UnexpectedApplication {
                detail: format!("expected version {}, found {version}", expected.version),
            });
        }
        Ok(ApplicationVerification {
            board: family,
            version,
            region: found.region,
            channel: found.channel,
        })
    }
}

fn identifies_expected_family(port: &str, expected: &ExpectedApplication) -> bool {
    matches_expected_family(&crate::identify(port), expected)
}

fn select_application_port(
    ports: &[String],
    original_port: &str,
    bootloader_port: &str,
    expected: &ExpectedApplication,
    mut identifies: impl FnMut(&str, &ExpectedApplication) -> bool,
) -> Result<Option<String>, DeviceFailure> {
    // Every path remains only a location candidate. Re-identify it before accepting it,
    // because a cable reset or reconnect may have put another device there.
    if ports.iter().any(|port| port == original_port) && identifies(original_port, expected) {
        return Ok(Some(original_port.to_string()));
    }

    // A T114 can leave its bootloader and return as the application without changing its COM
    // number. The old rediscovery filter excluded that path unconditionally, turning a
    // successful restore into a timeout even while Retinue was already answering there.
    if bootloader_port != original_port
        && ports.iter().any(|port| port == bootloader_port)
        && identifies(bootloader_port, expected)
    {
        return Ok(Some(bootloader_port.to_string()));
    }

    // Some boards return on an entirely new application port. Accept exactly one responsive
    // board of the family the immutable package expects. A COM number is never carried over as
    // identity, and an unrelated Retinue board must not turn a transfer into false success.
    let responsive: Vec<_> = ports
        .iter()
        .filter(|port| port.as_str() != bootloader_port && port.as_str() != original_port)
        .filter(|port| identifies(port, expected))
        .cloned()
        .collect();
    match responsive.as_slice() {
        [application_port] => Ok(Some(application_port.clone())),
        [] => Ok(None),
        ports => Err(DeviceFailure::UnexpectedPort {
            expected: original_port.into(),
            found: ports.join(", "),
        }),
    }
}

fn matches_expected_family(found: &crate::Found, expected: &ExpectedApplication) -> bool {
    found
        .board
        .as_ref()
        .and_then(crate::package::BoardFamily::from_board)
        .as_ref()
        == Some(&expected.board)
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ProcessFailure {
    #[error("{program} is not installed")]
    MissingHelper { program: String },
    #[error("{program} failed: {diagnostics}")]
    Failed {
        program: String,
        diagnostics: String,
    },
    #[error("{program} timed out")]
    Timeout { program: String },
    #[error(
        "{program} version mismatch: package requires {expected}, installed helper reports {found}"
    )]
    HelperVersionMismatch {
        program: String,
        expected: String,
        found: String,
    },
    #[error(
        "{program} digest mismatch: package requires {expected}, resolved helper hashes to {found}"
    )]
    HelperDigestMismatch {
        program: String,
        expected: String,
        found: String,
    },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DeviceFailure {
    #[error("bootloader did not appear within {0:?}")]
    Timeout(Duration),
    #[error("application did not answer within {0:?}")]
    ApplicationTimeout(Duration),
    #[error("device disappeared from {0}")]
    Disappeared(String),
    #[error("unexpected new port: expected {expected}, found {found}")]
    UnexpectedPort { expected: String, found: String },
    #[error("application on {0} was silent")]
    Silence(String),
    #[error("unexpected application: {detail}")]
    UnexpectedApplication { detail: String },
    #[error("{0}")]
    Other(String),
}

impl From<crate::Error> for DeviceFailure {
    fn from(error: crate::Error) -> Self {
        match error {
            crate::Error::NoBootloader(patience) => Self::Timeout(patience),
            crate::Error::Io(error) => Self::Other(error.to_string()),
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionStage {
    Preparing,
    EnteringBootloader,
    Transfer,
    Rebooting,
    VerifyingApplication,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryFacts {
    pub stage: ExecutionStage,
    pub transport: String,
    pub last_known_port: Option<String>,
    pub write_started: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlashEvent {
    Inspecting {
        device: String,
        package_id: String,
    },
    WaitingForOwnerAction {
        message: String,
    },
    EnteringBootloader,
    Rediscovering,
    Erasing,
    Writing {
        written: u64,
        total: u64,
    },
    VerifyingTransfer,
    Rebooting,
    VerifyingApplication,
    Complete {
        receipt: FlashReceipt,
    },
    ManualCheckRequired {
        receipt: FlashReceipt,
    },
    RecoveryRequired {
        facts: RecoveryFacts,
        instructions: RecoveryInstructions,
        receipt: FlashReceipt,
    },
    Refused {
        reasons: Vec<RefusalReason>,
    },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExecutionError {
    #[error("execution requires a serial device")]
    UnsupportedTransport,
    #[error("cannot write UF2 volume {volume}: {detail}")]
    VolumeWrite { volume: String, detail: String },
    #[error(
        "this executor cannot write the approved multi-part package; no device was opened or changed"
    )]
    UnsupportedPackageLayout,
    #[error("process failed: {0}")]
    Process(#[from] ProcessFailure),
    #[error("device failed: {0}")]
    Device(#[from] DeviceFailure),
    #[error("recovery required: {detail}")]
    RecoveryRequired {
        facts: RecoveryFacts,
        instructions: RecoveryInstructions,
        detail: String,
        receipt: FlashReceipt,
    },
}

pub const DEFAULT_PATIENCE: Duration = Duration::from_secs(12);
const APPLICATION_STARTUP_GRACE: Duration = Duration::from_secs(2);

pub fn execute_plan<P: ProcessRunner, D: DeviceRunner>(
    plan: &FlashPlan,
    package: &FlashPackage,
    process: &mut P,
    device: &mut D,
    patience: Duration,
    emit: &mut dyn FnMut(FlashEvent),
) -> Result<FlashReceipt, ExecutionError> {
    let executable = executable_layout(plan, package)?;
    let location = match &plan.observation().transport {
        DeviceTransport::SerialPort(port)
        | DeviceTransport::SerialDfuPort(port)
        | DeviceTransport::MountedVolume(port) => port.clone(),
    };
    emit(FlashEvent::Inspecting {
        device: location.clone(),
        package_id: plan.package().package_id.clone(),
    });
    for warning in plan.warnings() {
        if warning.requires_confirmation {
            emit(FlashEvent::WaitingForOwnerAction {
                message: warning.message.clone(),
            });
        }
    }
    if plan.route().uses_builtin_writer() {
        return execute_uf2_volume(plan, package, executable, &location, device, patience, emit);
    }
    let port = match &plan.observation().transport {
        DeviceTransport::SerialPort(port) | DeviceTransport::SerialDfuPort(port) => port.clone(),
        DeviceTransport::MountedVolume(_) => return Err(ExecutionError::UnsupportedTransport),
    };
    let helper = package.manifest().helper_for(plan.route()).ok_or_else(|| {
        ExecutionError::Process(ProcessFailure::Failed {
            program: plan.helper().into(),
            diagnostics: "package has no helper metadata for the selected route".into(),
        })
    })?;
    process.verify_helper(helper)?;

    let (bootloader_port, commands, command_bytes) = match (plan.route(), executable) {
        (FlashRoute::AdafruitDfu, ExecutableLayout::Container(part)) => {
            let dfu = if matches!(
                &plan.observation().transport,
                DeviceTransport::SerialDfuPort(_)
            ) {
                port.clone()
            } else {
                emit(FlashEvent::EnteringBootloader);
                let dfu = device.enter_bootloader(&port, patience).map_err(|error| {
                    recover(
                        plan,
                        emit,
                        ExecutionStage::EnteringBootloader,
                        &port,
                        false,
                        error.to_string(),
                    )
                })?;
                emit(FlashEvent::Rediscovering);
                dfu
            };
            (
                dfu.clone(),
                vec![adafruit_dfu::command(&dfu, part.path())],
                vec![part.declaration().write_bytes],
            )
        }
        (FlashRoute::EspRom, ExecutableLayout::Container(part)) => (
            port.clone(),
            vec![esp_rom::command(&port, part.path())],
            vec![part.declaration().write_bytes],
        ),
        (FlashRoute::EspRom, ExecutableLayout::SparseEsp(parts)) => (
            port.clone(),
            esp_rom::sparse_commands(&port, parts),
            parts
                .iter()
                .map(|part| part.declaration().write_bytes)
                .collect(),
        ),
        _ => return Err(ExecutionError::UnsupportedPackageLayout),
    };

    emit(FlashEvent::Erasing);
    let mut progress_events = Vec::new();
    let total_write_bytes = command_bytes.iter().sum::<u64>();
    let mut completed_write_bytes = 0;
    let mut write_started = false;
    let route_progress = match plan.route() {
        FlashRoute::AdafruitDfu => adafruit_dfu::progress,
        FlashRoute::EspRom => esp_rom::progress,
        FlashRoute::Uf2MassStorage => unreachable!("handled before external helper execution"),
    };
    for (arguments, part_write_bytes) in commands.iter().zip(command_bytes.iter().copied()) {
        let mut part_progress = false;
        let output = process
            .run(plan.helper(), arguments, &mut |progress| {
                part_progress = true;
                progress_events.push(scale_progress(
                    progress,
                    completed_write_bytes,
                    part_write_bytes,
                    total_write_bytes,
                ));
            })
            .map_err(|error| {
                if write_started || part_progress {
                    recover(
                        plan,
                        emit,
                        ExecutionStage::Transfer,
                        &port,
                        true,
                        error.to_string(),
                    )
                } else {
                    ExecutionError::Process(error)
                }
            })?;
        for line in output.diagnostics.lines() {
            if let Some(progress) = route_progress(line) {
                progress_events.push(scale_progress(
                    progress,
                    completed_write_bytes,
                    part_write_bytes,
                    total_write_bytes,
                ));
            }
        }
        write_started = true;
        completed_write_bytes += part_write_bytes;
    }
    for progress in progress_events {
        emit(FlashEvent::Writing {
            written: progress.written,
            total: progress.total,
        });
    }
    emit(FlashEvent::VerifyingTransfer);
    emit(FlashEvent::Rebooting);
    if let Some(instruction) = &package.manifest().expected_application.manual_check {
        let receipt = FlashReceipt::manual_check_required(
            plan,
            instruction.clone(),
            vec![ReceiptStage {
                name: "manual-check-required".into(),
                detail: Some("Every package part transferred and verified by the helper.".into()),
            }],
        );
        emit(FlashEvent::ManualCheckRequired {
            receipt: receipt.clone(),
        });
        return Ok(receipt);
    }
    let application_port = device
        .rediscover_application(
            &port,
            &bootloader_port,
            &package.manifest().expected_application,
            patience,
        )
        .map_err(|error| {
            recover(
                plan,
                emit,
                ExecutionStage::Rebooting,
                &port,
                true,
                error.to_string(),
            )
        })?;
    emit(FlashEvent::VerifyingApplication);
    let application = device
        .verify_application(&application_port, &package.manifest().expected_application)
        .map_err(|error| {
            recover(
                plan,
                emit,
                ExecutionStage::VerifyingApplication,
                &application_port,
                true,
                error.to_string(),
            )
        })?;
    let expected = &package.manifest().expected_application;
    if let Err(error) = crate::verify::verify_application(
        expected,
        &application,
        &package.manifest().regions,
        &package.manifest().channel_capabilities,
    ) {
        return Err(recover(
            plan,
            emit,
            ExecutionStage::VerifyingApplication,
            &application_port,
            true,
            error.to_string(),
        ));
    }
    let receipt = FlashReceipt::complete(
        plan,
        application,
        vec![ReceiptStage {
            name: "application-verified".into(),
            detail: None,
        }],
    );
    emit(FlashEvent::Complete {
        receipt: receipt.clone(),
    });
    Ok(receipt)
}

enum ExecutableLayout<'a> {
    Container(&'a VerifiedPackagePart),
    SparseEsp(&'a [VerifiedPackagePart]),
    Uf2(&'a VerifiedPackagePart),
}

fn executable_layout<'a>(
    plan: &FlashPlan,
    package: &'a FlashPackage,
) -> Result<ExecutableLayout<'a>, ExecutionError> {
    match (plan.route(), package.parts()) {
        (FlashRoute::AdafruitDfu, [part])
            if matches!(part.declaration().format, PayloadFormat::NrfDfuZip) =>
        {
            Ok(ExecutableLayout::Container(part))
        }
        (FlashRoute::EspRom, [part])
            if matches!(part.declaration().format, PayloadFormat::EspflashElf) =>
        {
            Ok(ExecutableLayout::Container(part))
        }
        (FlashRoute::Uf2MassStorage, [part])
            if matches!(part.declaration().format, PayloadFormat::Uf2) =>
        {
            Ok(ExecutableLayout::Uf2(part))
        }
        (FlashRoute::EspRom, parts)
            if parts.len() == 3
                && parts[0].declaration().kind == FirmwarePartKind::Bootloader
                && parts[1].declaration().kind == FirmwarePartKind::PartitionTable
                && parts[2].declaration().kind == FirmwarePartKind::Application
                && parts.iter().all(|part| {
                    matches!(part.declaration().format, PayloadFormat::RawBinary)
                        && part.declaration().offset.is_some()
                }) =>
        {
            Ok(ExecutableLayout::SparseEsp(parts))
        }
        _ => Err(ExecutionError::UnsupportedPackageLayout),
    }
}

fn execute_uf2_volume<D: DeviceRunner>(
    plan: &FlashPlan,
    package: &FlashPackage,
    executable: ExecutableLayout<'_>,
    location: &str,
    device: &mut D,
    patience: Duration,
    emit: &mut dyn FnMut(FlashEvent),
) -> Result<FlashReceipt, ExecutionError> {
    let DeviceTransport::MountedVolume(volume) = &plan.observation().transport else {
        return Err(ExecutionError::UnsupportedTransport);
    };
    let ExecutableLayout::Uf2(part) = executable else {
        return Err(ExecutionError::UnsupportedPackageLayout);
    };
    let destination =
        uf2_destination(volume, part).map_err(|detail| ExecutionError::VolumeWrite {
            volume: volume.clone(),
            detail,
        })?;

    let write = write_uf2_file(&destination, part.bytes()).map_err(|error| {
        recover(
            plan,
            emit,
            ExecutionStage::Transfer,
            location,
            true,
            format!("could not write {}: {error}", destination.display()),
        )
    })?;
    if write.bytes != part.declaration().byte_length {
        return Err(recover(
            plan,
            emit,
            ExecutionStage::Transfer,
            location,
            true,
            format!(
                "wrote {} bytes to {}, package requires {}",
                write.bytes,
                destination.display(),
                part.declaration().byte_length
            ),
        ));
    }
    emit(FlashEvent::Writing {
        written: part.declaration().write_bytes,
        total: part.declaration().write_bytes,
    });
    emit(FlashEvent::VerifyingTransfer);
    emit(FlashEvent::Rebooting);
    let transfer_detail = if write.ejected_after_write {
        format!(
            "The UF2 volume ejected after Linkboy wrote all {} verified package bytes to {}; that is the bootloader's transfer acknowledgement.",
            part.declaration().byte_length,
            destination.display()
        )
    } else {
        format!(
            "The built-in UF2 volume writer created {} with {} verified package bytes.",
            destination.display(),
            part.declaration().byte_length
        )
    };
    if let Some(instruction) = &package.manifest().expected_application.manual_check {
        let receipt = FlashReceipt::manual_check_required(
            plan,
            instruction.clone(),
            vec![ReceiptStage {
                name: "manual-check-required".into(),
                detail: Some(format!(
                    "{transfer_detail} The upstream application check remains required."
                )),
            }],
        );
        emit(FlashEvent::ManualCheckRequired {
            receipt: receipt.clone(),
        });
        return Ok(receipt);
    }

    let expected = &package.manifest().expected_application;
    let application_port = device
        .rediscover_application("", "", expected, patience)
        .map_err(|error| {
            recover(
                plan,
                emit,
                ExecutionStage::Rebooting,
                location,
                true,
                error.to_string(),
            )
        })?;
    emit(FlashEvent::VerifyingApplication);
    let application = device
        .verify_application(&application_port, expected)
        .map_err(|error| {
            recover(
                plan,
                emit,
                ExecutionStage::VerifyingApplication,
                &application_port,
                true,
                error.to_string(),
            )
        })?;
    if let Err(error) = crate::verify::verify_application(
        expected,
        &application,
        &package.manifest().regions,
        &package.manifest().channel_capabilities,
    ) {
        return Err(recover(
            plan,
            emit,
            ExecutionStage::VerifyingApplication,
            &application_port,
            true,
            error.to_string(),
        ));
    }
    let receipt = FlashReceipt::complete(
        plan,
        application,
        vec![ReceiptStage {
            name: "uf2-application-verified".into(),
            detail: Some(transfer_detail),
        }],
    );
    emit(FlashEvent::Complete {
        receipt: receipt.clone(),
    });
    Ok(receipt)
}

fn uf2_destination(volume: &str, part: &VerifiedPackagePart) -> Result<PathBuf, String> {
    let root = Path::new(volume);
    if !root.is_dir() {
        return Err("mounted volume is not an accessible directory".into());
    }
    let file_name = part
        .path()
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "package UF2 has no file name".to_string())?;
    if !file_name
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".uf2")
    {
        return Err("package UF2 file name must end in .uf2".into());
    }
    let destination = root.join(file_name);
    if destination.exists() {
        return Err(format!(
            "refusing to overwrite existing {}",
            destination.display()
        ));
    }
    Ok(destination)
}

struct Uf2Write {
    bytes: u64,
    ejected_after_write: bool,
}

fn write_uf2_file(destination: &Path, bytes: &[u8]) -> std::io::Result<Uf2Write> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    file.write_all(bytes)?;
    match file.sync_all() {
        Ok(()) => Ok(Uf2Write {
            bytes: bytes.len() as u64,
            ejected_after_write: false,
        }),
        // Adafruit UF2 bootloaders deliberately leave the mass-storage bus after a complete
        // file write, to apply the image and reboot. Windows has reported that normal
        // acknowledgement as both ERROR_DEV_NOT_EXIST (55) and ERROR_DEVICE_DOES_NOT_EXIST
        // (433) while flushing the just-written file.
        Err(error) if uf2_volume_ejected_after_write(destination, &error) => Ok(Uf2Write {
            bytes: bytes.len() as u64,
            ejected_after_write: true,
        }),
        Err(error) => Err(error),
    }
}

fn uf2_volume_ejected_after_write(destination: &Path, error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(55 | 433))
        || destination.parent().is_some_and(|root| !root.is_dir())
}

fn scale_progress(
    progress: ProcessProgress,
    completed: u64,
    part_total: u64,
    total: u64,
) -> ProcessProgress {
    let written = if progress.total == 0 {
        0
    } else {
        progress.written.saturating_mul(part_total) / progress.total
    }
    .min(part_total);
    ProcessProgress {
        written: completed + written,
        total,
    }
}

fn recover(
    plan: &FlashPlan,
    emit: &mut dyn FnMut(FlashEvent),
    stage: ExecutionStage,
    port: &str,
    write_started: bool,
    detail: String,
) -> ExecutionError {
    let facts = RecoveryFacts {
        stage: stage.clone(),
        transport: match &plan.observation().transport {
            DeviceTransport::SerialPort(port) => format!("serial:{port}"),
            DeviceTransport::SerialDfuPort(port) => format!("serial-dfu:{port}"),
            DeviceTransport::MountedVolume(volume) => format!("volume:{volume}"),
        },
        last_known_port: Some(port.to_string()),
        write_started,
        detail: detail.clone(),
    };
    let instructions = RecoveryInstructions {
        before_write: plan.recovery_before_write().into(),
        after_failure: plan.recovery_after_failure().into(),
    };
    let receipt = FlashReceipt::recovery_required(
        plan,
        vec![ReceiptStage {
            name: format!("recovery-{stage:?}"),
            detail: Some(detail.clone()),
        }],
    );
    emit(FlashEvent::RecoveryRequired {
        facts: facts.clone(),
        instructions: instructions.clone(),
        receipt: receipt.clone(),
    });
    ExecutionError::RecoveryRequired {
        facts,
        instructions,
        detail,
        receipt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{
        BoardSelection, DeviceObservation, EvidenceConfidence, FirmwareState, HardwareFacts,
    };
    use crate::package::{
        BoardFamily, ExpectedApplication, FirmwarePartKind, FlashPackageManifest, FlashRange,
        PACKAGE_SCHEMA, PackagePart, PackagePayload, PackageTarget, PayloadFormat, StateImpact,
    };
    use crate::plan::{CompatibilityFact, PackageIdentity, PlanWarning};

    struct MockProcess {
        result: Result<ProcessOutput, ProcessFailure>,
        progress: Vec<ProcessProgress>,
    }

    impl ProcessRunner for MockProcess {
        fn run(
            &mut self,
            _program: &str,
            _args: &[String],
            progress: &mut dyn FnMut(ProcessProgress),
        ) -> Result<ProcessOutput, ProcessFailure> {
            for value in self.progress.clone() {
                progress(value);
            }
            self.result.clone()
        }
    }

    #[derive(Default)]
    struct RecordingProcess {
        calls: Vec<Vec<String>>,
    }

    impl ProcessRunner for RecordingProcess {
        fn run(
            &mut self,
            _program: &str,
            args: &[String],
            progress: &mut dyn FnMut(ProcessProgress),
        ) -> Result<ProcessOutput, ProcessFailure> {
            self.calls.push(args.to_vec());
            progress(ProcessProgress {
                written: 100,
                total: 100,
            });
            Ok(ProcessOutput {
                diagnostics: "write 100%".into(),
            })
        }
    }

    struct MockDevice {
        bootloader: Result<String, DeviceFailure>,
        application: Result<String, DeviceFailure>,
        verification: Result<ApplicationVerification, DeviceFailure>,
    }

    impl DeviceRunner for MockDevice {
        fn enter_bootloader(
            &mut self,
            _current_port: &str,
            _patience: Duration,
        ) -> Result<String, DeviceFailure> {
            self.bootloader.clone()
        }

        fn rediscover_application(
            &mut self,
            _original_port: &str,
            _bootloader_port: &str,
            _expected: &ExpectedApplication,
            _patience: Duration,
        ) -> Result<String, DeviceFailure> {
            self.application.clone()
        }

        fn verify_application(
            &mut self,
            _application_port: &str,
            _expected: &ExpectedApplication,
        ) -> Result<ApplicationVerification, DeviceFailure> {
            self.verification.clone()
        }
    }

    fn package(route: FlashRoute) -> FlashPackage {
        let manual_check = matches!(route, FlashRoute::Uf2MassStorage)
            .then(|| "Exercise the upstream interface.".into());
        package_with_manual_check(route, manual_check)
    }

    fn package_with_manual_check(route: FlashRoute, manual_check: Option<String>) -> FlashPackage {
        let bytes = match route {
            FlashRoute::Uf2MassStorage => test_uf2_bytes(),
            FlashRoute::AdafruitDfu | FlashRoute::EspRom => b"payload".to_vec(),
        };
        let (family, processor, bootloader, format, revision) = match route {
            FlashRoute::AdafruitDfu => (
                BoardFamily::T114,
                crate::package::ProcessorKind::Nrf52840,
                "s140-v6",
                PayloadFormat::NrfDfuZip,
                "2.x",
            ),
            FlashRoute::EspRom => (
                BoardFamily::HeltecV4,
                crate::package::ProcessorKind::Esp32S3,
                "esp-rom",
                PayloadFormat::EspflashElf,
                "4.2",
            ),
            FlashRoute::Uf2MassStorage => (
                BoardFamily::T114,
                crate::package::ProcessorKind::Nrf52840,
                "adafruit-uf2-0.9.0",
                PayloadFormat::Uf2,
                "2.x",
            ),
        };
        let manifest = FlashPackageManifest {
            schema: PACKAGE_SCHEMA,
            package_id: "test".into(),
            display_name: "Test".into(),
            version: "1".into(),
            publisher: "Test".into(),
            helpers: vec![crate::package::HelperRequirement {
                route: route.clone(),
                program: route.helper().into(),
                version: match route {
                    FlashRoute::AdafruitDfu => "0.5.3.post16",
                    FlashRoute::EspRom => "4.5.0",
                    FlashRoute::Uf2MassStorage => "0.0.1",
                }
                .into(),
                binary_sha256: None,
                artifacts: Vec::new(),
                license: "test".into(),
                source_url: "https://example.invalid/helper".into(),
                notice: "Test helper notice".into(),
            }],
            payload: Some(PackagePayload {
                path: match route {
                    FlashRoute::Uf2MassStorage => "payload.uf2",
                    FlashRoute::AdafruitDfu | FlashRoute::EspRom => "payload",
                }
                .into(),
                format,
                byte_length: bytes.len() as u64,
                sha256: crate::package::sha256_hex(&bytes),
                write_bytes: match route {
                    FlashRoute::Uf2MassStorage => 4,
                    FlashRoute::AdafruitDfu | FlashRoute::EspRom => bytes.len() as u64,
                },
            }),
            parts: Vec::new(),
            targets: vec![PackageTarget {
                family: family.clone(),
                revision: revision.into(),
                processor,
                flash_size: match route {
                    FlashRoute::Uf2MassStorage => 1024 * 1024,
                    FlashRoute::AdafruitDfu | FlashRoute::EspRom => 4 * 1024 * 1024,
                },
                bootloader: bootloader.into(),
                route: route.clone(),
            }],
            write_ranges: match route {
                FlashRoute::Uf2MassStorage => vec![FlashRange {
                    start: 0x26000,
                    length: 4,
                }],
                FlashRoute::AdafruitDfu | FlashRoute::EspRom => vec![FlashRange {
                    start: 0,
                    length: 1,
                }],
            },
            preserved_ranges: match route {
                FlashRoute::Uf2MassStorage => vec![FlashRange {
                    start: 0x26004,
                    length: 1,
                }],
                FlashRoute::AdafruitDfu | FlashRoute::EspRom => vec![FlashRange {
                    start: 1,
                    length: 1,
                }],
            },
            regions: vec!["US915".into()],
            channel_capabilities: vec!["modem".into(), "node".into(), "rnode".into()],
            state_impact: StateImpact::Preserved,
            expected_application: ExpectedApplication {
                board: family,
                version: "0.0.1".into(),
                manual_check,
            },
            license: "MPL-2.0".into(),
            notices: "Notices".into(),
            source_revision: "test".into(),
            source_url: "https://example.invalid/source".into(),
            origin_url: "https://example.invalid/package".into(),
            publisher_signature: None,
            recovery: RecoveryInstructions {
                before_write: "Keep cable attached.".into(),
                after_failure: "Use bootloader recovery.".into(),
            },
        };
        let payload_path = match route {
            FlashRoute::Uf2MassStorage => "payload.uf2",
            FlashRoute::AdafruitDfu | FlashRoute::EspRom => "payload",
        };
        FlashPackage::from_parts(manifest, "manifest", payload_path, bytes).unwrap()
    }

    fn test_uf2_bytes() -> Vec<u8> {
        let mut block = vec![0_u8; 512];
        let word = |block: &mut [u8], offset: usize, value: u32| {
            block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        };
        word(&mut block, 0, 0x0A32_4655_u32);
        word(&mut block, 4, 0x9E5D_5157_u32);
        word(&mut block, 8, 0x0000_2000_u32);
        word(&mut block, 12, 0x26000_u32);
        word(&mut block, 16, 4_u32);
        word(&mut block, 20, 0_u32);
        word(&mut block, 24, 1_u32);
        word(&mut block, 28, crate::uf2::NRF52840_FAMILY_ID);
        block[32..36].copy_from_slice(b"UF2!");
        word(&mut block, 508, 0x0AB1_6F30_u32);
        block
    }

    fn sparse_esp_package() -> FlashPackage {
        let bootloader = b"bootloader".to_vec();
        let partition_table = b"partition-table".to_vec();
        let application = b"application".to_vec();
        let parts = [
            (
                "bootloader.bin",
                FirmwarePartKind::Bootloader,
                0,
                bootloader,
            ),
            (
                "partition-table.bin",
                FirmwarePartKind::PartitionTable,
                0x8000,
                partition_table,
            ),
            (
                "application.bin",
                FirmwarePartKind::Application,
                0x10000,
                application,
            ),
        ];
        let manifest = FlashPackageManifest {
            schema: PACKAGE_SCHEMA,
            package_id: "upstream.hopspot-v4".into(),
            display_name: "Upstream Hopspot for Heltec V4".into(),
            version: "test".into(),
            publisher: "Upstream".into(),
            helpers: vec![crate::package::HelperRequirement {
                route: FlashRoute::EspRom,
                program: "espflash".into(),
                version: "4.5.0".into(),
                binary_sha256: None,
                artifacts: Vec::new(),
                license: "MIT OR Apache-2.0".into(),
                source_url: "https://example.invalid/espflash".into(),
                notice: "Test helper notice".into(),
            }],
            payload: None,
            parts: parts
                .iter()
                .map(|(path, kind, offset, bytes)| PackagePart {
                    kind: kind.clone(),
                    path: (*path).into(),
                    format: PayloadFormat::RawBinary,
                    offset: Some(*offset),
                    byte_length: bytes.len() as u64,
                    sha256: crate::package::sha256_hex(bytes),
                    write_bytes: bytes.len() as u64,
                })
                .collect(),
            targets: vec![PackageTarget {
                family: BoardFamily::HeltecV4,
                revision: "4.2".into(),
                processor: crate::package::ProcessorKind::Esp32S3,
                flash_size: 4 * 1024 * 1024,
                bootloader: "esp-rom".into(),
                route: FlashRoute::EspRom,
            }],
            write_ranges: Vec::new(),
            preserved_ranges: vec![FlashRange {
                start: 0xd000,
                length: 0x1000,
            }],
            regions: vec!["US915".into()],
            channel_capabilities: vec!["modem".into()],
            state_impact: StateImpact::Unknown,
            expected_application: ExpectedApplication {
                board: BoardFamily::HeltecV4,
                version: "test".into(),
                manual_check: Some("Exercise the upstream interface.".into()),
            },
            license: "MPL-2.0".into(),
            notices: "Test notices".into(),
            source_revision: "test".into(),
            source_url: "https://example.invalid/source".into(),
            origin_url: "https://example.invalid/package".into(),
            publisher_signature: None,
            recovery: RecoveryInstructions {
                before_write: "Keep the cable attached.".into(),
                after_failure: "Enter the ROM loader again.".into(),
            },
        };
        FlashPackage::from_verified_parts(
            manifest,
            "manifest",
            parts
                .into_iter()
                .map(|(path, _, _, bytes)| (path.into(), bytes))
                .collect(),
        )
        .unwrap()
    }

    fn plan(route: FlashRoute) -> FlashPlan {
        plan_with_transport(route, DeviceTransport::SerialPort("COM7".into()))
    }

    fn plan_with_transport(route: FlashRoute, transport: DeviceTransport) -> FlashPlan {
        let (family, processor, bootloader, revision) = match route {
            FlashRoute::AdafruitDfu => (
                BoardFamily::T114,
                crate::package::ProcessorKind::Nrf52840,
                "s140-v6",
                "2.x",
            ),
            FlashRoute::EspRom => (
                BoardFamily::HeltecV4,
                crate::package::ProcessorKind::Esp32S3,
                "esp-rom",
                "4.2",
            ),
            FlashRoute::Uf2MassStorage => (
                BoardFamily::T114,
                crate::package::ProcessorKind::Nrf52840,
                "adafruit-uf2-0.9.0",
                "2.x",
            ),
        };
        FlashPlan::for_test(
            DeviceObservation {
                transport,
                status_reply: None,
                hardware: HardwareFacts {
                    processor: Some(processor),
                    flash_size: Some(4 * 1024 * 1024),
                    bootloader: Some(bootloader.into()),
                    loader_route: None,
                    bootloader_usb: None,
                },
                selected_board: Some(BoardSelection::owner_confirmed(family.clone(), revision)),
                firmware: FirmwareState::Retinue {
                    family: family.clone(),
                },
                confidence: EvidenceConfidence::OwnerConfirmed,
                contradictions: Vec::new(),
            },
            PackageIdentity {
                package_id: "test".into(),
                display_name: "Test".into(),
                version: "1".into(),
                parts: vec![crate::PackagePartIdentity {
                    kind: crate::FirmwarePartKind::Application,
                    offset: None,
                    byte_length: 1,
                    sha256: "a".repeat(64),
                }],
                publisher_signature: None,
            },
            BoardSelection::owner_confirmed(family, revision),
            route,
            vec![],
            vec![],
            StateImpact::Preserved,
            vec![CompatibilityFact {
                name: "board".into(),
                value: "confirmed".into(),
                source: "test".into(),
            }],
            vec![PlanWarning {
                message: "warning".into(),
                requires_confirmation: false,
            }],
            "before".into(),
            "after".into(),
        )
    }

    fn uf2_volume_plan(volume: String) -> FlashPlan {
        FlashPlan::for_test(
            DeviceObservation {
                transport: DeviceTransport::MountedVolume(volume),
                status_reply: None,
                hardware: HardwareFacts {
                    processor: Some(crate::package::ProcessorKind::Nrf52840),
                    flash_size: Some(1024 * 1024),
                    bootloader: Some("adafruit-uf2-0.9.0".into()),
                    loader_route: Some("uf2-mass-storage".into()),
                    bootloader_usb: None,
                },
                selected_board: Some(BoardSelection::owner_confirmed(BoardFamily::T114, "2.x")),
                firmware: FirmwareState::Bootloader,
                confidence: EvidenceConfidence::OwnerConfirmed,
                contradictions: Vec::new(),
            },
            PackageIdentity {
                package_id: "test".into(),
                display_name: "Test".into(),
                version: "1".into(),
                parts: vec![crate::PackagePartIdentity {
                    kind: crate::FirmwarePartKind::Application,
                    offset: None,
                    byte_length: 512,
                    sha256: crate::package::sha256_hex(&test_uf2_bytes()),
                }],
                publisher_signature: None,
            },
            BoardSelection::owner_confirmed(BoardFamily::T114, "2.x"),
            FlashRoute::Uf2MassStorage,
            vec![FlashRange {
                start: 0x26000,
                length: 4,
            }],
            vec![FlashRange {
                start: 0x26004,
                length: 1,
            }],
            StateImpact::Unknown,
            vec![CompatibilityFact {
                name: "bootloader".into(),
                value: "UF2".into(),
                source: "test".into(),
            }],
            Vec::new(),
            "before".into(),
            "after".into(),
        )
    }

    fn success_device() -> MockDevice {
        MockDevice {
            bootloader: Ok("DFU1".into()),
            application: Ok("COM7".into()),
            verification: Ok(ApplicationVerification {
                board: BoardFamily::HeltecV4,
                version: "0.0.1".into(),
                region: Some("US915".into()),
                channel: Some("rnode".into()),
            }),
        }
    }

    #[test]
    fn success_emits_complete_only_after_application_verification() {
        let plan = plan(FlashRoute::EspRom);
        let package = package(FlashRoute::EspRom);
        let mut process = MockProcess {
            result: Ok(ProcessOutput {
                diagnostics: "write 100%".into(),
            }),
            progress: vec![ProcessProgress {
                written: 100,
                total: 100,
            }],
        };
        let mut device = success_device();
        let mut events = Vec::new();
        let receipt = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .unwrap();
        assert_eq!(receipt.result, crate::receipt::ReceiptResult::Complete);
        let complete = events
            .iter()
            .position(|event| matches!(event, FlashEvent::Complete { .. }))
            .unwrap();
        let verify = events
            .iter()
            .position(|event| matches!(event, FlashEvent::VerifyingApplication))
            .unwrap();
        assert!(verify < complete);
    }

    #[test]
    fn an_explicit_t114_dfu_port_skips_bootloader_entry() {
        let plan = plan_with_transport(
            FlashRoute::AdafruitDfu,
            DeviceTransport::SerialDfuPort("COM10".into()),
        );
        let package = package(FlashRoute::AdafruitDfu);
        let mut process = RecordingProcess::default();
        let mut device = MockDevice {
            bootloader: Err(DeviceFailure::Other(
                "an already-DFU route must not enter the bootloader".into(),
            )),
            application: Ok("COM10".into()),
            verification: Ok(ApplicationVerification {
                board: BoardFamily::T114,
                version: "0.0.1".into(),
                region: Some("US915".into()),
                channel: Some("modem".into()),
            }),
        };
        let mut events = Vec::new();

        let receipt = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect("the selected DFU port should execute directly");

        assert_eq!(receipt.result, crate::ReceiptResult::Complete);
        assert_eq!(process.calls.len(), 1);
        assert_eq!(process.calls[0][5], "COM10");
        assert!(!events.iter().any(|event| matches!(
            event,
            FlashEvent::EnteringBootloader | FlashEvent::Rediscovering
        )));
    }

    #[test]
    fn missing_helper_is_preserved_as_a_structured_process_failure() {
        let plan = plan(FlashRoute::EspRom);
        let package = package(FlashRoute::EspRom);
        let mut process = MockProcess {
            result: Err(ProcessFailure::MissingHelper {
                program: "espflash".into(),
            }),
            progress: vec![],
        };
        let mut device = success_device();
        let mut events = Vec::new();
        let error = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect_err("missing helper must stop before transfer");
        assert!(matches!(
            error,
            ExecutionError::Process(ProcessFailure::MissingHelper { .. })
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FlashEvent::Complete { .. }))
        );
    }

    #[test]
    fn helper_timeout_is_preserved_as_a_structured_process_failure() {
        let plan = plan(FlashRoute::EspRom);
        let package = package(FlashRoute::EspRom);
        let mut process = MockProcess {
            result: Err(ProcessFailure::Timeout {
                program: "espflash".into(),
            }),
            progress: vec![],
        };
        let mut device = success_device();
        let mut events = Vec::new();
        let error = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect_err("a helper timeout before progress must stop the transfer");
        assert!(matches!(
            error,
            ExecutionError::Process(ProcessFailure::Timeout { .. })
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FlashEvent::Complete { .. }))
        );
    }

    #[test]
    fn helper_failure_after_progress_requires_recovery() {
        let plan = plan(FlashRoute::EspRom);
        let package = package(FlashRoute::EspRom);
        let mut process = MockProcess {
            result: Err(ProcessFailure::Failed {
                program: "espflash".into(),
                diagnostics: "write failed".into(),
            }),
            progress: vec![ProcessProgress {
                written: 50,
                total: 100,
            }],
        };
        let mut device = success_device();
        let mut events = Vec::new();
        let error = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect_err("partial transfer must require recovery");
        assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
        assert!(events.iter().any(|event| matches!(
            event,
            FlashEvent::RecoveryRequired {
                facts: RecoveryFacts {
                    write_started: true,
                    ..
                },
                ..
            }
        )));
    }

    #[test]
    fn disappearing_device_and_post_write_silence_are_recovery_events() {
        let plan = plan(FlashRoute::EspRom);
        let package = package(FlashRoute::EspRom);
        let mut process = MockProcess {
            result: Ok(ProcessOutput {
                diagnostics: String::new(),
            }),
            progress: vec![ProcessProgress {
                written: 100,
                total: 100,
            }],
        };
        let mut device = MockDevice {
            bootloader: Ok("DFU1".into()),
            application: Err(DeviceFailure::Disappeared("COM7".into())),
            verification: Err(DeviceFailure::Silence("COM7".into())),
        };
        let mut events = Vec::new();
        let error = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect_err("lost application must require recovery");
        assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
        assert!(events.iter().any(|event| matches!(
            event,
            FlashEvent::RecoveryRequired {
                facts: RecoveryFacts {
                    stage: ExecutionStage::Rebooting,
                    ..
                },
                ..
            }
        )));
    }

    #[test]
    fn unexpected_application_port_is_a_recovery_event() {
        let plan = plan(FlashRoute::EspRom);
        let package = package(FlashRoute::EspRom);
        let mut process = MockProcess {
            result: Ok(ProcessOutput {
                diagnostics: "write 100%".into(),
            }),
            progress: vec![ProcessProgress {
                written: 100,
                total: 100,
            }],
        };
        let mut device = MockDevice {
            bootloader: Ok("DFU1".into()),
            application: Err(DeviceFailure::UnexpectedPort {
                expected: "COM7".into(),
                found: "COM9".into(),
            }),
            verification: Err(DeviceFailure::Silence("COM9".into())),
        };
        let mut events = Vec::new();
        let error = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect_err("a contradictory application port must require recovery");
        assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
        assert!(events.iter().any(|event| matches!(
            event,
            FlashEvent::RecoveryRequired {
                facts: RecoveryFacts {
                    stage: ExecutionStage::Rebooting,
                    last_known_port: Some(port),
                    ..
                },
                ..
            } if port == "COM7"
        )));
    }

    #[test]
    fn wrong_application_after_successful_transfer_requires_recovery() {
        let plan = plan(FlashRoute::EspRom);
        let package = package(FlashRoute::EspRom);
        let mut process = MockProcess {
            result: Ok(ProcessOutput {
                diagnostics: String::new(),
            }),
            progress: vec![ProcessProgress {
                written: 100,
                total: 100,
            }],
        };
        let mut device = MockDevice {
            bootloader: Ok("DFU1".into()),
            application: Ok("COM7".into()),
            verification: Ok(ApplicationVerification {
                board: BoardFamily::HeltecV4,
                version: "0.0.2".into(),
                region: None,
                channel: None,
            }),
        };
        let mut events = Vec::new();
        let error = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect_err("wrong application must not be complete");
        assert!(matches!(error, ExecutionError::RecoveryRequired { .. }));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FlashEvent::Complete { .. }))
        );
    }

    #[test]
    fn route_commands_select_the_new_t114_port() {
        let command = adafruit_dfu::command("COM9", std::path::Path::new("firmware.zip"));
        assert_eq!(command[5], "COM9");
        let command = esp_rom::command("COM7", std::path::Path::new("firmware.elf"));
        assert_eq!(command[2], "COM7");
    }

    #[test]
    fn rediscovery_rejects_an_unrelated_retinue_family() {
        let expected = ExpectedApplication {
            board: BoardFamily::T114,
            version: "0.0.1".into(),
            manual_check: None,
        };
        let v4 = crate::Found {
            port: "COM6".into(),
            board: Some(crate::Board::HeltecV4),
            banner: "tulle/heltec-v4 phy online".into(),
            region: None,
            channel: None,
        };
        let t114 = crate::Found {
            port: "COM10".into(),
            board: Some(crate::Board::T114),
            banner: "tulle/t114 phy online".into(),
            region: None,
            channel: None,
        };
        assert!(!matches_expected_family(&v4, &expected));
        assert!(matches_expected_family(&t114, &expected));
    }

    #[test]
    fn rediscovery_accepts_application_on_former_bootloader_port() {
        let expected = ExpectedApplication {
            board: BoardFamily::T114,
            version: "0.0.1".into(),
            manual_check: None,
        };
        let ports = vec!["COM10".to_string()];
        let application = select_application_port(&ports, "COM3", "COM10", &expected, |port, _| {
            port == "COM10"
        })
        .expect("one expected application port is unambiguous");

        assert_eq!(application.as_deref(), Some("COM10"));
    }

    #[test]
    fn sparse_esp_package_writes_every_part_then_requires_its_own_manual_check() {
        let plan = plan(FlashRoute::EspRom);
        let package = sparse_esp_package();
        let mut process = RecordingProcess::default();
        let mut device = success_device();
        let mut events = Vec::new();
        let receipt = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect("a validated sparse package should execute every part");
        assert_eq!(
            receipt.result,
            crate::receipt::ReceiptResult::ManualCheckRequired
        );
        assert_eq!(process.calls.len(), 3);
        assert!(
            process.calls[0]
                .windows(2)
                .any(|pair| pair == ["--before", "usb-reset"])
        );
        assert!(
            process.calls[1]
                .windows(2)
                .any(|pair| pair == ["--before", "no-reset"])
        );
        assert!(
            process.calls[2]
                .windows(2)
                .any(|pair| pair == ["--after", "watchdog-reset"])
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FlashEvent::ManualCheckRequired { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FlashEvent::VerifyingApplication))
        );
    }

    #[test]
    fn uf2_volume_writer_copies_the_verified_file_without_an_external_helper() {
        let volume =
            std::env::temp_dir().join(format!("linkboy-uf2-volume-{}", std::process::id()));
        std::fs::create_dir(&volume).unwrap();
        let package = package(FlashRoute::Uf2MassStorage);
        let plan = uf2_volume_plan(volume.to_string_lossy().into_owned());
        let mut process = MockProcess {
            result: Err(ProcessFailure::MissingHelper {
                program: "must not run".into(),
            }),
            progress: Vec::new(),
        };
        let mut device = success_device();
        let mut events = Vec::new();
        let receipt = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect("built-in writer should not need an external helper");
        let copied = std::fs::read(volume.join("payload.uf2")).unwrap();
        assert_eq!(copied, package.parts()[0].bytes());
        assert_eq!(
            receipt.result,
            crate::receipt::ReceiptResult::ManualCheckRequired
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FlashEvent::ManualCheckRequired { .. }))
        );
        std::fs::remove_dir_all(volume).unwrap();
    }

    #[test]
    fn retinue_uf2_install_completes_only_after_application_verification() {
        let volume =
            std::env::temp_dir().join(format!("linkboy-retinue-uf2-volume-{}", std::process::id()));
        std::fs::create_dir(&volume).unwrap();
        let package = package_with_manual_check(FlashRoute::Uf2MassStorage, None);
        let plan = uf2_volume_plan(volume.to_string_lossy().into_owned());
        let mut process = MockProcess {
            result: Err(ProcessFailure::MissingHelper {
                program: "must not run".into(),
            }),
            progress: Vec::new(),
        };
        let mut device = MockDevice {
            bootloader: Err(DeviceFailure::Other("must not enter bootloader".into())),
            application: Ok("COM10".into()),
            verification: Ok(ApplicationVerification {
                board: BoardFamily::T114,
                version: "0.0.1".into(),
                region: Some("US915".into()),
                channel: Some("rnode".into()),
            }),
        };
        let mut events = Vec::new();

        let receipt = execute_plan(
            &plan,
            &package,
            &mut process,
            &mut device,
            Duration::from_secs(1),
            &mut |event| events.push(event),
        )
        .expect("a Retinue UF2 install should verify the returned application");

        assert_eq!(receipt.result, crate::receipt::ReceiptResult::Complete);
        let verify = events
            .iter()
            .position(|event| matches!(event, FlashEvent::VerifyingApplication))
            .unwrap();
        let complete = events
            .iter()
            .position(|event| matches!(event, FlashEvent::Complete { .. }))
            .unwrap();
        assert!(verify < complete);
        std::fs::remove_dir_all(volume).unwrap();
    }

    #[test]
    fn uf2_disconnect_after_full_write_is_a_bootloader_acknowledgement() {
        let destination = std::env::temp_dir().join("payload.uf2");
        for code in [55, 433] {
            let error = std::io::Error::from_raw_os_error(code);
            assert!(uf2_volume_ejected_after_write(&destination, &error));
        }
    }
}
