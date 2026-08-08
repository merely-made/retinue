//! The linkboy, as a terminal.
//!
//! ```text
//! linkboy list                    what is on this machine's ports
//! linkboy flash PORT IMAGE [t114|v4]   put an image on the board at PORT
//!                                      name the board when it cannot say what it is
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
    Board, Error, converse, enter_bootloader, have_tool, identify, ports, require_image, run,
};

const BOOTLOADER_PATIENCE: Duration = Duration::from_secs(12);

fn usage() -> &'static str {
    "usage:\n  \
     linkboy list\n  \
     linkboy ask PORT LINE...\n  \
     linkboy flash PORT IMAGE [t114|v4]\n  \
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
        Some("flash") => {
            let port = args.next().ok_or_else(|| bad_usage("flash needs a PORT"))?;
            let image = args
                .next()
                .ok_or_else(|| bad_usage("flash needs an IMAGE"))?;
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

fn list() -> Result<(), Error> {
    for port in ports()? {
        println!("{}", identify(&port).describe());
    }
    Ok(())
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
