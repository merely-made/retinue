//! The linkboy, as a terminal.
//!
//! ```text
//! linkboy list                    what is on this machine's ports
//! linkboy flash PORT IMAGE        put an image on the board at PORT
//! linkboy bootloader PORT         send a T114 to its bootloader and name the new port
//! ```
//!
//! Flashing shells out to the tool each board needs — `adafruit-nrfutil` for the T114's
//! serial DFU, `espflash` for the ESP ROM loader — rather than reimplementing either. What
//! linkboy adds is the part that is fiddly by hand and undocumented in one place: knowing
//! which board it is talking to, sending it to its bootloader, finding the port it comes back
//! on, and refusing to write anything until all of that is settled.

use std::time::Duration;

use linkboy::{Board, Error, enter_bootloader, have_tool, identify, ports, require_image, run};

const BOOTLOADER_PATIENCE: Duration = Duration::from_secs(12);

fn usage() -> &'static str {
    "usage:\n  \
     linkboy list\n  \
     linkboy flash PORT IMAGE\n  \
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
            flash(&port, &image)
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

fn flash(port: &str, image: &str) -> Result<(), Error> {
    // Everything that can be checked before something irreversible starts, is.
    require_image(image)?;
    let found = identify(port);
    let board = found
        .board
        .ok_or_else(|| Error::NotOurs(port.to_string()))?;

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
