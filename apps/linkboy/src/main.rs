//! The linkboy, as a terminal.
//!
//! ```text
//! linkboy list                    what is on this machine's ports
//! linkboy inspect PACKAGE            verify and explain a package
//! linkboy catalog INDEX           verify a public package index
//! linkboy plan DEVICE PACKAGE [BOARD@REVISION]
//!                                      produce a refusal or immutable flash plan
//! linkboy flash DEVICE PACKAGE [BOARD@REVISION] [--receipt PATH]
//!                                      execute an accepted package plan
//! linkboy flash-raw PORT IMAGE [t114|v4]
//!                                      expert-only bench route for a raw image
//! linkboy bootloader PORT         send a T114 to its bootloader and name the new port
//! ```
//!
//! Flashing shells out to the tool each board needs — `adafruit-nrfutil` for the T114's
//! serial DFU, `espflash` for the ESP ROM loader — rather than reimplementing either. What
//! linkboy adds is the part that is fiddly by hand and undocumented in one place: knowing
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
     linkboy flash DEVICE PACKAGE [BOARD@REVISION] [--receipt PATH]\n  \
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
            let mut process = SystemProcessRunner;
            if matches!(found.board, Some(Board::HeltecV4)) {
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
            if let Some((family, revision)) = selection {
                observation = observation.confirm_board(family, revision);
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
            let mut receipt_path = None;
            while let Some(value) = args.next() {
                if value == "--receipt" {
                    receipt_path = Some(
                        args.next()
                            .ok_or_else(|| bad_usage("--receipt needs a PATH"))?,
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
            let mut process = SystemProcessRunner;
            if matches!(found.board, Some(Board::HeltecV4)) {
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
            if let Some((family, revision)) = selection {
                observation = observation.confirm_board(family, revision);
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
