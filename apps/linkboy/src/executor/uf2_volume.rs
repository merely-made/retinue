//! The built-in UF2 mass-storage writer route.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::device::DeviceTransport;
use crate::package::{FlashPackage, VerifiedPackagePart};
use crate::plan::FlashPlan;
use crate::receipt::{FlashReceipt, ReceiptStage};

use super::event::recover;
use super::run::ExecutableLayout;
use super::{DeviceRunner, ExecutionError, ExecutionStage, FlashEvent};

pub(super) fn execute_uf2_volume<D: DeviceRunner>(
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

pub(super) fn uf2_volume_ejected_after_write(destination: &Path, error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(55 | 433))
        || destination.parent().is_some_and(|root| !root.is_dir())
}
