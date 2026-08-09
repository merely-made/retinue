//! Structured execution beneath CLI and graphical faces.

use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::device::DeviceTransport;
use crate::package::{
    ExpectedApplication, FlashPackage, FlashRoute, HelperRequirement, RecoveryInstructions,
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
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &mut self,
        program: &str,
        args: &[String],
        progress: &mut dyn FnMut(ProcessProgress),
    ) -> Result<ProcessOutput, ProcessFailure> {
        let output = Command::new(program)
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
        crate::helper::verify_installed(self, requirement)
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
        patience: Duration,
    ) -> Result<String, DeviceFailure> {
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            let Ok(ports) = crate::ports() else {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            };

            // The original path is only a location candidate. Re-identify it before accepting
            // it, because a cable reset or a user reconnect may have put another device there.
            if ports.iter().any(|port| port == original_port)
                && crate::identify(original_port).board.is_some()
            {
                return Ok(original_port.to_string());
            }

            // Some boards return on a new application port after leaving their loader. Probe
            // each candidate and accept exactly one responsive Retinue board. A COM number is
            // never carried over as identity, and two responsive candidates are ambiguous.
            let responsive: Vec<_> = ports
                .into_iter()
                .filter(|port| port != bootloader_port && port != original_port)
                .filter(|port| crate::identify(port).board.is_some())
                .collect();
            match responsive.as_slice() {
                [application_port] => return Ok(application_port.clone()),
                [] => {}
                ports => {
                    return Err(DeviceFailure::UnexpectedPort {
                        expected: original_port.into(),
                        found: ports.join(", "),
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(DeviceFailure::Timeout(patience))
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
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DeviceFailure {
    #[error("bootloader did not appear within {0:?}")]
    Timeout(Duration),
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

pub fn execute_plan<P: ProcessRunner, D: DeviceRunner>(
    plan: &FlashPlan,
    package: &FlashPackage,
    process: &mut P,
    device: &mut D,
    patience: Duration,
    emit: &mut dyn FnMut(FlashEvent),
) -> Result<FlashReceipt, ExecutionError> {
    let port = match &plan.observation().transport {
        DeviceTransport::SerialPort(port) => port.clone(),
        DeviceTransport::MountedVolume(_) => return Err(ExecutionError::UnsupportedTransport),
    };
    emit(FlashEvent::Inspecting {
        device: port.clone(),
        package_id: plan.package().package_id.clone(),
    });
    for warning in plan.warnings() {
        if warning.requires_confirmation {
            emit(FlashEvent::WaitingForOwnerAction {
                message: warning.message.clone(),
            });
        }
    }
    let helper = package.manifest().helper_for(plan.route()).ok_or_else(|| {
        ExecutionError::Process(ProcessFailure::Failed {
            program: plan.helper().into(),
            diagnostics: "package has no helper metadata for the selected route".into(),
        })
    })?;
    process.verify_helper(helper)?;

    let (bootloader_port, arguments) = match plan.route() {
        FlashRoute::AdafruitDfu => {
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
            (
                dfu.clone(),
                adafruit_dfu::command(&dfu, package.payload_path()),
            )
        }
        FlashRoute::EspRom => (
            port.clone(),
            esp_rom::command(&port, package.payload_path()),
        ),
    };

    emit(FlashEvent::Erasing);
    let mut progress_events = Vec::new();
    let output = process
        .run(plan.helper(), &arguments, &mut |progress| {
            progress_events.push(progress)
        })
        .map_err(|error| {
            let write_started = !progress_events.is_empty();
            if write_started {
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
    let route_progress = match plan.route() {
        FlashRoute::AdafruitDfu => adafruit_dfu::progress,
        FlashRoute::EspRom => esp_rom::progress,
    };
    for line in output.diagnostics.lines() {
        if let Some(progress) = route_progress(line) {
            progress_events.push(progress);
        }
    }
    for progress in progress_events {
        emit(FlashEvent::Writing {
            written: progress.written,
            total: progress.total,
        });
    }
    emit(FlashEvent::VerifyingTransfer);
    emit(FlashEvent::Rebooting);
    let application_port = device
        .rediscover_application(&port, &bootloader_port, patience)
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
        BoardFamily, ExpectedApplication, FlashPackageManifest, FlashRange, PACKAGE_SCHEMA,
        PackagePayload, PackageTarget, PayloadFormat, StateImpact,
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
        let bytes = b"payload".to_vec();
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
                }
                .into(),
                license: "test".into(),
                source_url: "https://example.invalid/helper".into(),
                notice: "Test helper notice".into(),
            }],
            payload: PackagePayload {
                path: "payload".into(),
                format,
                byte_length: bytes.len() as u64,
                sha256: crate::package::sha256_hex(&bytes),
                write_bytes: bytes.len() as u64,
            },
            targets: vec![PackageTarget {
                family: family.clone(),
                revision: revision.into(),
                processor,
                flash_size: 4 * 1024 * 1024,
                bootloader: bootloader.into(),
                route: route.clone(),
            }],
            write_ranges: vec![FlashRange {
                start: 0,
                length: 1,
            }],
            preserved_ranges: vec![FlashRange {
                start: 1,
                length: 1,
            }],
            regions: vec!["US915".into()],
            channel_capabilities: vec!["modem".into(), "node".into(), "rnode".into()],
            state_impact: StateImpact::Preserved,
            expected_application: ExpectedApplication {
                board: family,
                version: "0.0.1".into(),
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
        FlashPackage::from_parts(manifest, "manifest", "payload", bytes).unwrap()
    }

    fn plan(route: FlashRoute) -> FlashPlan {
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
        };
        FlashPlan::for_test(
            DeviceObservation {
                transport: DeviceTransport::SerialPort("COM7".into()),
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
                sha256: "a".repeat(64),
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
}
