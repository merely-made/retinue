use std::time::Duration;

use crate::device::DeviceTransport;
use crate::package::{
    FirmwarePartKind, FlashPackage, FlashRoute, PayloadFormat, VerifiedPackagePart,
};
use crate::plan::FlashPlan;
use crate::receipt::{FlashReceipt, ReceiptStage};
use crate::route::{adafruit_dfu, esp_rom};

use super::event::recover;
use super::uf2_volume::execute_uf2_volume;
use super::{
    DeviceRunner, ExecutionError, ExecutionStage, FlashEvent, ProcessFailure, ProcessProgress,
    ProcessRunner,
};

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

pub(super) enum ExecutableLayout<'a> {
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

fn scale_progress(
    progress: ProcessProgress,
    completed: u64,
    part_total: u64,
    total: u64,
) -> ProcessProgress {
    let written = progress
        .written
        .saturating_mul(part_total)
        .checked_div(progress.total)
        .unwrap_or(0)
        .min(part_total);
    ProcessProgress {
        written: completed + written,
        total,
    }
}
