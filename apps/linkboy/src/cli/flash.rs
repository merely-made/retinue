//! Device helpers and the expert raw-image flash route.

use linkboy::{
    Board, DeviceObservation, Error, enter_bootloader, have_tool, identify, ports, require_image,
    run,
};

use super::BOOTLOADER_PATIENCE;

pub(super) fn list() -> Result<(), Error> {
    for port in ports()? {
        println!("{}", identify(&port).describe());
    }
    Ok(())
}

pub(super) fn uf2_volume_observation(volume: &str) -> Result<DeviceObservation, Error> {
    linkboy::t114_uf2_observation(volume)
        .map(|(observation, _)| observation)
        .map_err(|error| Error::ToolFailed {
            tool: "linkboy",
            message: error.to_string(),
        })
}

pub(super) fn t114_loader_snapshot(volume: &str) -> Result<linkboy::T114LoaderSnapshot, Error> {
    linkboy::t114_loader_snapshot_from_volume(volume).map_err(|error| Error::ToolFailed {
        tool: "linkboy",
        message: error.to_string(),
    })
}

pub(super) fn ensure_post_write_recovery_matches(
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

pub(super) fn flash(port: &str, image: &str, declared: Option<Board>) -> Result<(), Error> {
    // Everything that can be checked before something irreversible starts, is.
    require_image(image)?;
    // A declared board wins over the probe: a board running foreign firmware, or none, answers
    // nothing, and recovering such a board is the job.
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
