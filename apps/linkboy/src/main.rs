#![forbid(unsafe_code)]

//! The linkboy, as a terminal.
//!
//! ```text
//! linkboy list                    what is on this machine's ports
//! linkboy inspect PACKAGE            verify and explain a package
//! linkboy catalog INDEX           verify a public package index
//! linkboy plan DEVICE PACKAGE [BOARD@REVISION]
//!                                      produce a refusal or immutable flash plan
//! linkboy flash DEVICE PACKAGE [BOARD@REVISION] [--loader-snapshot PATH] [--receipt PATH]
//!                                      execute an accepted package plan
//! linkboy flash-volume VOLUME PACKAGE BOARD@REVISION [--receipt PATH]
//!                                      execute an admitted T114 UF2 package
//! linkboy capture-t114-loader VOLUME PATH
//!                                      capture the HT-n5262 UF2 and SoftDevice record
//! linkboy make-uf2 BIN UF2 BASE FAMILY
//!                                      reproducibly package a raw application as UF2
//! linkboy verify-recovery PORT PACKAGE BOARD@REVISION RECOVERY --loader-snapshot PATH [--receipt PATH]
//!                                      verify a completed post-write recovery without writing
//! linkboy flash-raw PORT IMAGE [t114|v4]
//!                                      expert-only bench route for a raw image
//! linkboy bootloader PORT         send a T114 to its bootloader and name the new port
//! ```
//!
//! Public T114 packages use Linkboy's built-in UF2 writer. V4 packages use an admitted,
//! platform-specific `espflash` release for the ESP ROM loader; serial DFU remains an expert
//! T114 recovery route. Linkboy adds the part that is fiddly by hand and undocumented in one place: knowing
//! which board it is talking to, sending it to its bootloader, finding the port it comes back
//! on, and refusing to write anything until all of that is settled.

use std::time::Duration;

use linkboy::{
    Board, BoardFamily, DeviceObservation, Error, FlashEvent, FlashPackage, LiveDeviceRunner,
    SystemProcessRunner, converse, enter_bootloader, execute_plan, have_tool, identify, plan_flash,
    ports, require_image, run,
};

const BOOTLOADER_PATIENCE: Duration = Duration::from_secs(12);

fn usage() -> &'static str {
    "usage:\n  \
     linkboy list\n  \
     linkboy ask PORT LINE...\n  \
     linkboy inspect PACKAGE\n  \
     linkboy catalog INDEX\n  \
     linkboy plan DEVICE PACKAGE [BOARD@REVISION]\n  \
     linkboy flash DEVICE PACKAGE [BOARD@REVISION] [--loader-snapshot PATH] [--receipt PATH]\n  \
     linkboy flash-volume VOLUME PACKAGE BOARD@REVISION [--receipt PATH]\n  \
     linkboy capture-t114-loader VOLUME PATH\n  \
     linkboy make-uf2 BIN UF2 BASE FAMILY\n  \
     linkboy verify-recovery PORT PACKAGE BOARD@REVISION RECOVERY --loader-snapshot PATH [--receipt PATH]\n  \
     linkboy flash-raw PORT IMAGE [t114|v4]\n  \
     linkboy bootloader PORT"
}

fn main() {
    if let Err(error) = run_command() {
        eprintln!("linkboy: {error}");
        std::process::exit(1);
    }
}

fn run_command() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("list") => list(),
        Some("inspect") => {
            let package = args
                .next()
                .ok_or_else(|| bad_usage("inspect needs a PACKAGE"))?;
            if args.next().is_some() {
                return Err(bad_usage("inspect accepts one PACKAGE"));
            }
            println!("{}", FlashPackage::load(package)?.describe());
            Ok(())
        }
        Some("catalog") => {
            let index_path = args
                .next()
                .ok_or_else(|| bad_usage("catalog needs an INDEX"))?;
            if args.next().is_some() {
                return Err(bad_usage("catalog accepts one INDEX"));
            }
            let index = linkboy::PackageIndex::load(&index_path)?;
            index.verify_packages(&index_path)?;
            println!("{}", index.describe());
            Ok(())
        }
        Some("plan") => {
            let device = args
                .next()
                .ok_or_else(|| bad_usage("plan needs a DEVICE"))?;
            let package_path = args
                .next()
                .ok_or_else(|| bad_usage("plan needs a PACKAGE"))?;
            let selection = args
                .next()
                .map(|value| parse_board_selection(&value))
                .transpose()?;
            if args.next().is_some() {
                return Err(bad_usage("plan accepts DEVICE PACKAGE [BOARD@REVISION]"));
            }
            let package = FlashPackage::load(package_path)?;
            let found = identify(&device);
            let mut observation = DeviceObservation::from_found(&found);
            let mut process = SystemProcessRunner::default();
            if let Some((family, revision)) = selection {
                observation = observation.confirm_board(family, revision);
            }
            if linkboy::needs_esp_rom_probe(&observation) {
                let facts = linkboy::route::esp_rom::discover(&mut process, &device)
                    .map_err(|error| Error::Execution(linkboy::ExecutionError::Process(error)))?;
                observation = observation.with_hardware(linkboy::HardwareFacts {
                    processor: facts.processor.clone(),
                    flash_size: facts.flash_size,
                    bootloader: facts.bootloader.clone(),
                    loader_route: Some("esp-rom".into()),
                    bootloader_usb: Some(facts),
                });
            }
            let plan = plan_flash(&observation, &package).map_err(Error::Refused)?;
            println!("{}", plan.describe());
            Ok(())
        }
        Some("flash") => {
            let device = args
                .next()
                .ok_or_else(|| bad_usage("flash needs a DEVICE"))?;
            let package_path = args
                .next()
                .ok_or_else(|| bad_usage("flash needs a PACKAGE"))?;
            let mut selection = None;
            let mut loader_snapshot_path = None;
            let mut receipt_path = None;
            while let Some(value) = args.next() {
                if value == "--receipt" {
                    receipt_path = Some(
                        args.next()
                            .ok_or_else(|| bad_usage("--receipt needs a PATH"))?,
                    );
                } else if value == "--loader-snapshot" {
                    loader_snapshot_path = Some(
                        args.next()
                            .ok_or_else(|| bad_usage("--loader-snapshot needs a PATH"))?,
                    );
                } else if selection.is_none() {
                    selection = Some(parse_board_selection(&value)?);
                } else {
                    return Err(bad_usage(
                        "flash accepts DEVICE PACKAGE [BOARD@REVISION] [--receipt PATH]",
                    ));
                }
            }
            let package = FlashPackage::load(package_path)?;
            let found = identify(&device);
            let mut observation = DeviceObservation::from_found(&found);
            let mut process = SystemProcessRunner::default();
            if let Some((family, revision)) = selection {
                observation = observation.confirm_board(family, revision);
            }
            if let Some(path) = loader_snapshot_path {
                let selected = observation.selected_board.as_ref().ok_or_else(|| {
                    bad_usage("--loader-snapshot requires an explicit t114@REVISION selection")
                })?;
                if selected.family != BoardFamily::T114 {
                    return Err(bad_usage(
                        "--loader-snapshot is only valid for t114@REVISION",
                    ));
                }
                let snapshot = linkboy::T114LoaderSnapshot::from_json(&path).map_err(|error| {
                    Error::ToolFailed {
                        tool: "linkboy",
                        message: error.to_string(),
                    }
                })?;
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
                let facts = linkboy::route::esp_rom::discover(&mut process, &device)
                    .map_err(|error| Error::Execution(linkboy::ExecutionError::Process(error)))?;
                observation = observation.with_hardware(linkboy::HardwareFacts {
                    processor: facts.processor.clone(),
                    flash_size: facts.flash_size,
                    bootloader: facts.bootloader.clone(),
                    loader_route: Some("esp-rom".into()),
                    bootloader_usb: Some(facts),
                });
            }
            let plan = match plan_flash(&observation, &package) {
                Ok(plan) => plan,
                Err(refusal) => {
                    render_event(FlashEvent::Refused {
                        reasons: refusal.reasons.clone(),
                    });
                    return Err(Error::Refused(refusal));
                }
            };
            let mut runner = LiveDeviceRunner;
            let mut render = render_event;
            let result = execute_plan(
                &plan,
                &package,
                &mut process,
                &mut runner,
                linkboy::executor::DEFAULT_PATIENCE,
                &mut render,
            );
            match result {
                Ok(receipt) => {
                    if let Some(path) = receipt_path {
                        receipt.save_json(path)?;
                    }
                    Ok(())
                }
                Err(error) => {
                    if let Some(path) = receipt_path {
                        if let linkboy::ExecutionError::RecoveryRequired { receipt, .. } = &error {
                            receipt.save_json(path)?;
                        }
                    }
                    Err(error.into())
                }
            }
        }
        Some("flash-volume") => {
            let volume = args
                .next()
                .ok_or_else(|| bad_usage("flash-volume needs a VOLUME"))?;
            let package_path = args
                .next()
                .ok_or_else(|| bad_usage("flash-volume needs a PACKAGE"))?;
            let selection = args
                .next()
                .ok_or_else(|| bad_usage("flash-volume needs BOARD@REVISION"))
                .and_then(|value| parse_board_selection(&value))?;
            if selection.0 != BoardFamily::T114 {
                return Err(bad_usage(
                    "flash-volume currently admits only a T114 UF2 bootloader volume",
                ));
            }
            let mut receipt_path = None;
            while let Some(value) = args.next() {
                if value == "--receipt" {
                    receipt_path = Some(
                        args.next()
                            .ok_or_else(|| bad_usage("--receipt needs a PATH"))?,
                    );
                } else {
                    return Err(bad_usage(
                        "flash-volume accepts VOLUME PACKAGE BOARD@REVISION [--receipt PATH]",
                    ));
                }
            }
            let package = FlashPackage::load(package_path)?;
            let observation =
                uf2_volume_observation(&volume)?.confirm_board(selection.0, selection.1);
            let plan = match plan_flash(&observation, &package) {
                Ok(plan) => plan,
                Err(refusal) => {
                    render_event(FlashEvent::Refused {
                        reasons: refusal.reasons.clone(),
                    });
                    return Err(Error::Refused(refusal));
                }
            };
            let mut process = SystemProcessRunner::default();
            let mut runner = LiveDeviceRunner;
            let mut render = render_event;
            let result = execute_plan(
                &plan,
                &package,
                &mut process,
                &mut runner,
                linkboy::executor::DEFAULT_PATIENCE,
                &mut render,
            );
            match result {
                Ok(receipt) => {
                    if let Some(path) = receipt_path {
                        receipt.save_json(path)?;
                    }
                    Ok(())
                }
                Err(error) => {
                    if let Some(path) = receipt_path {
                        if let linkboy::ExecutionError::RecoveryRequired { receipt, .. } = &error {
                            receipt.save_json(path)?;
                        }
                    }
                    Err(error.into())
                }
            }
        }
        Some("capture-t114-loader") => {
            let volume = args
                .next()
                .ok_or_else(|| bad_usage("capture-t114-loader needs a VOLUME"))?;
            let path = args
                .next()
                .ok_or_else(|| bad_usage("capture-t114-loader needs a PATH"))?;
            if args.next().is_some() {
                return Err(bad_usage("capture-t114-loader accepts VOLUME PATH"));
            }
            let snapshot = t114_loader_snapshot(&volume)?;
            snapshot
                .save_json(&path)
                .map_err(|error| Error::ToolFailed {
                    tool: "linkboy",
                    message: error.to_string(),
                })?;
            println!("captured T114 loader record from {volume} into {path}");
            Ok(())
        }
        Some("make-uf2") => {
            let input = args
                .next()
                .ok_or_else(|| bad_usage("make-uf2 needs a BIN input"))?;
            let output = args
                .next()
                .ok_or_else(|| bad_usage("make-uf2 needs a UF2 output"))?;
            let base = args
                .next()
                .ok_or_else(|| bad_usage("make-uf2 needs a BASE address"))
                .and_then(|value| parse_u32(&value, "BASE"))?;
            let family = args
                .next()
                .ok_or_else(|| bad_usage("make-uf2 needs a FAMILY id"))
                .and_then(|value| parse_u32(&value, "FAMILY"))?;
            if args.next().is_some() {
                return Err(bad_usage("make-uf2 accepts BIN UF2 BASE FAMILY"));
            }
            let application = std::fs::read(&input).map_err(|error| Error::ToolFailed {
                tool: "linkboy",
                message: format!("could not read {input}: {error}"),
            })?;
            let uf2 = linkboy::encode_application(&application, base, family).map_err(|error| {
                Error::ToolFailed {
                    tool: "linkboy",
                    message: error.to_string(),
                }
            })?;
            if std::path::Path::new(&output).exists() {
                return Err(Error::ToolFailed {
                    tool: "linkboy",
                    message: format!("refusing to overwrite {output}"),
                });
            }
            std::fs::write(&output, &uf2).map_err(|error| Error::ToolFailed {
                tool: "linkboy",
                message: format!("could not write {output}: {error}"),
            })?;
            println!(
                "wrote {} UF2 bytes from {} application bytes",
                uf2.len(),
                application.len()
            );
            Ok(())
        }
        Some("verify-recovery") => {
            let port = args
                .next()
                .ok_or_else(|| bad_usage("verify-recovery needs a PORT"))?;
            let package_path = args
                .next()
                .ok_or_else(|| bad_usage("verify-recovery needs a PACKAGE"))?;
            let selection = args
                .next()
                .ok_or_else(|| bad_usage("verify-recovery needs BOARD@REVISION"))
                .and_then(|value| parse_board_selection(&value))?;
            let recovery_path = args
                .next()
                .ok_or_else(|| bad_usage("verify-recovery needs a RECOVERY receipt"))?;
            let mut snapshot_path = None;
            let mut receipt_path = None;
            while let Some(value) = args.next() {
                match value.as_str() {
                    "--loader-snapshot" => {
                        snapshot_path = Some(
                            args.next()
                                .ok_or_else(|| bad_usage("--loader-snapshot needs a PATH"))?,
                        )
                    }
                    "--receipt" => {
                        receipt_path = Some(
                            args.next()
                                .ok_or_else(|| bad_usage("--receipt needs a PATH"))?,
                        )
                    }
                    _ => {
                        return Err(bad_usage(
                            "verify-recovery accepts PORT PACKAGE BOARD@REVISION RECOVERY --loader-snapshot PATH [--receipt PATH]",
                        ));
                    }
                }
            }
            if selection.0 != BoardFamily::T114 {
                return Err(bad_usage(
                    "verify-recovery currently requires an explicit t114@REVISION selection",
                ));
            }
            let snapshot_path = snapshot_path.ok_or_else(|| {
                bad_usage("verify-recovery requires the captured --loader-snapshot PATH")
            })?;
            let package = FlashPackage::load(package_path)?;
            let snapshot =
                linkboy::T114LoaderSnapshot::from_json(snapshot_path).map_err(|error| {
                    Error::ToolFailed {
                        tool: "linkboy",
                        message: error.to_string(),
                    }
                })?;
            let facts = snapshot.serial_dfu_observation();
            let observation = DeviceObservation::from_found(&identify(&port))
                .confirm_board(selection.0, selection.1)
                .with_hardware(linkboy::HardwareFacts {
                    processor: facts.processor.clone(),
                    flash_size: facts.flash_size,
                    bootloader: facts.bootloader.clone(),
                    loader_route: Some("captured-t114-loader-snapshot".into()),
                    bootloader_usb: Some(facts),
                });
            let plan = plan_flash(&observation, &package).map_err(Error::Refused)?;
            let recovery = linkboy::FlashReceipt::load_json(&recovery_path)?;
            ensure_post_write_recovery_matches(&recovery, &plan)?;
            let mut runner = LiveDeviceRunner;
            let application = linkboy::DeviceRunner::verify_application(
                &mut runner,
                &port,
                &package.manifest().expected_application,
            )
            .map_err(linkboy::ExecutionError::Device)?;
            linkboy::verify_application(
                &package.manifest().expected_application,
                &application,
                &package.manifest().regions,
                &package.manifest().channel_capabilities,
            )
            .map_err(|error| Error::ToolFailed {
                tool: "linkboy",
                message: format!("recovered application did not match package: {error}"),
            })?;
            let receipt = linkboy::FlashReceipt::complete(
                &plan,
                application,
                vec![linkboy::ReceiptStage {
                    name: "recovery-application-verified".into(),
                    detail: Some(format!(
                        "Verified the returned application on {port} after the matching post-write recovery receipt {}.",
                        recovery_path
                    )),
                }],
            );
            if let Some(path) = receipt_path {
                receipt.save_json(path)?;
            }
            println!("complete");
            println!(
                "{}",
                receipt.to_json().unwrap_or_else(|error| error.to_string())
            );
            Ok(())
        }
        Some("flash-raw") => {
            let port = args
                .next()
                .ok_or_else(|| bad_usage("flash-raw needs a PORT"))?;
            let image = args
                .next()
                .ok_or_else(|| bad_usage("flash-raw needs an IMAGE"))?;
            // An optional third word says what the board is, for when it cannot say so
            // itself: running stock RNode, half-flashed, or simply wedged. Naming it is the
            // operator taking responsibility for a claim linkboy could not check.
            let declared = match args.next().as_deref() {
                None => None,
                Some("t114") => Some(Board::T114),
                Some("v4") => Some(Board::HeltecV4),
                Some(other) => {
                    return Err(bad_usage(&format!(
                        "unknown board {other}, want t114 or v4"
                    )));
                }
            };
            flash(&port, &image, declared)
        }
        // The board's whole probe vocabulary, reachable from the tool that already knows how
        // to open its port and wait out its settling. Several lines go in one session,
        // because opening the port is the slow part.
        Some("ask") => {
            let port = args.next().ok_or_else(|| bad_usage("ask needs a PORT"))?;
            let lines: Vec<String> = args.collect();
            if lines.is_empty() {
                return Err(bad_usage("ask needs at least one LINE"));
            }
            let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
            for answer in converse(&port, &borrowed)? {
                print!("{answer}");
            }
            Ok(())
        }
        Some("bootloader") => {
            let port = args
                .next()
                .ok_or_else(|| bad_usage("bootloader needs a PORT"))?;
            let fresh = enter_bootloader(&port, BOOTLOADER_PATIENCE)?;
            println!("{port} rebooted; its bootloader is on {fresh}");
            Ok(())
        }
        _ => {
            println!("{}", usage());
            Ok(())
        }
    }
}

fn bad_usage(what: &str) -> Error {
    Error::ToolFailed {
        tool: "linkboy",
        message: format!("{what}\n{}", usage()),
    }
}

fn parse_u32(value: &str, label: &str) -> Result<u32, Error> {
    let parsed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|digits| u32::from_str_radix(digits, 16))
        .unwrap_or_else(|| value.parse::<u32>());
    parsed.map_err(|_| {
        bad_usage(&format!(
            "{label} must be a 32-bit decimal or 0x hexadecimal value"
        ))
    })
}

fn parse_board_selection(value: &str) -> Result<(BoardFamily, String), Error> {
    let (family, revision) = value
        .split_once('@')
        .ok_or_else(|| bad_usage("BOARD must be t114@REVISION or v4@REVISION"))?;
    let family = match family {
        "t114" => BoardFamily::T114,
        "v4" | "heltec-v4" => BoardFamily::HeltecV4,
        other => return Err(bad_usage(&format!("unknown board family {other}"))),
    };
    if revision.trim().is_empty() {
        return Err(bad_usage("BOARD revision cannot be empty"));
    }
    Ok((family, revision.to_string()))
}

fn list() -> Result<(), Error> {
    for port in ports()? {
        println!("{}", identify(&port).describe());
    }
    Ok(())
}

fn uf2_volume_observation(volume: &str) -> Result<DeviceObservation, Error> {
    linkboy::t114_uf2_observation(volume)
        .map(|(observation, _)| observation)
        .map_err(|error| Error::ToolFailed {
            tool: "linkboy",
            message: error.to_string(),
        })
}

fn t114_loader_snapshot(volume: &str) -> Result<linkboy::T114LoaderSnapshot, Error> {
    linkboy::t114_loader_snapshot_from_volume(volume).map_err(|error| Error::ToolFailed {
        tool: "linkboy",
        message: error.to_string(),
    })
}

fn ensure_post_write_recovery_matches(
    recovery: &linkboy::FlashReceipt,
    plan: &linkboy::FlashPlan,
) -> Result<(), Error> {
    let post_write_verification =
        matches!(&recovery.result, linkboy::ReceiptResult::RecoveryRequired)
            && recovery
                .stages
                .iter()
                .any(|stage| stage.name == "recovery-VerifyingApplication");
    let matches_plan = recovery.package_id == plan.package().package_id
        && recovery.package_parts == plan.package().parts
        && recovery.board == plan.board().family
        && recovery.board_revision == plan.board().revision
        && recovery.route == *plan.route();
    if post_write_verification && matches_plan {
        Ok(())
    } else {
        Err(Error::ToolFailed {
            tool: "linkboy",
            message:
                "recovery receipt is not a matching post-write application-verification recovery"
                    .into(),
        })
    }
}

fn render_event(event: FlashEvent) {
    match event {
        FlashEvent::Inspecting { device, package_id } => {
            println!("inspecting {device} with package {package_id}")
        }
        FlashEvent::WaitingForOwnerAction { message } => println!("owner action: {message}"),
        FlashEvent::EnteringBootloader => println!("entering bootloader"),
        FlashEvent::Rediscovering => println!("rediscovering device"),
        FlashEvent::Erasing => println!("erasing"),
        FlashEvent::Writing { written, total } => println!("writing {written}/{total}"),
        FlashEvent::VerifyingTransfer => println!("verifying transfer"),
        FlashEvent::Rebooting => println!("rebooting"),
        FlashEvent::VerifyingApplication => println!("verifying application"),
        FlashEvent::Complete { receipt } => {
            println!("complete");
            println!(
                "{}",
                receipt.to_json().unwrap_or_else(|error| error.to_string())
            );
        }
        FlashEvent::ManualCheckRequired { receipt } => {
            println!("manual check required");
            println!(
                "{}",
                receipt.to_json().unwrap_or_else(|error| error.to_string())
            );
        }
        FlashEvent::RecoveryRequired {
            facts,
            instructions,
            receipt,
        } => {
            println!("recovery required: {}", facts.detail);
            println!("before write: {}", instructions.before_write);
            println!("after failure: {}", instructions.after_failure);
            println!(
                "{}",
                receipt.to_json().unwrap_or_else(|error| error.to_string())
            );
        }
        FlashEvent::Refused { reasons } => {
            println!("refused:");
            for reason in reasons {
                println!("- {reason}");
            }
        }
    }
}

fn flash(port: &str, image: &str, declared: Option<Board>) -> Result<(), Error> {
    // Everything that can be checked before something irreversible starts, is.
    require_image(image)?;
    // A declared board wins over the probe. A board running somebody else's firmware, or
    // none at all, answers nothing, and refusing to flash it was exactly backwards:
    // recovering a board that has stopped talking is the job.
    let board = match declared {
        Some(board) => {
            println!("{port}: taking your word for it, {board:?}");
            board
        }
        None => identify(port)
            .board
            .ok_or_else(|| Error::NotOurs(port.to_string()))?,
    };

    match board {
        Board::Unknown(line) => Err(Error::UnknownBoard(line)),

        Board::HeltecV4 => {
            if !have_tool("espflash") {
                return Err(Error::MissingTool {
                    tool: "espflash",
                    board: Board::HeltecV4,
                });
            }
            println!("{port}: Heltec V4, flashing over the ESP ROM loader");
            let output = run("espflash", &["flash", "-p", port, image])?;
            print!("{output}");
            println!("{port}: flashed");
            Ok(())
        }

        Board::T114 => {
            if !have_tool("adafruit-nrfutil") {
                return Err(Error::MissingTool {
                    tool: "adafruit-nrfutil",
                    board: Board::T114,
                });
            }
            println!("{port}: T114, sending it to its bootloader");
            // The board re-enumerates as a different port, so the one to flash is discovered
            // rather than assumed.
            let dfu = enter_bootloader(port, BOOTLOADER_PATIENCE)?;
            println!("{port}: bootloader is on {dfu}, writing {image}");
            let output = run(
                "adafruit-nrfutil",
                &[
                    "dfu",
                    "serial",
                    "-pkg",
                    image,
                    "-p",
                    &dfu,
                    "-b",
                    "115200",
                    "--singlebank",
                ],
            )?;
            print!("{output}");
            println!("{port}: flashed; it should come back on its application port");
            Ok(())
        }
    }
}
