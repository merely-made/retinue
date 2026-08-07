//! Linkboy: the firmware and link-update tool of the retinue family.
//!
//! A linkboy carried the light that saw travellers through dark streets; this one carries
//! firmware over the link. It is the friendly face of the stock-hardware, user-flash posture:
//! the tool a person runs once, nervously, to put a retinue image on a board they own.
//!
//! # Boards identify themselves, rather than being identified
//!
//! The obvious way to tell a T114 from a V4 is USB vendor and product IDs. This does not do
//! that, and the reason is worth stating: a VID/PID says what chip enumerated, not what
//! firmware is on it. A board in its bootloader enumerates as something else entirely, a
//! board running a stranger's firmware enumerates the same as ours, and a board wired onto a
//! different carrier enumerates as whatever that carrier is.
//!
//! So linkboy asks. Every retinue image answers `status` with a banner naming itself, in
//! every channel, at any frame boundary — that probe was built for a bench and turns out to
//! be exactly the identification a flasher needs. A board that answers is ours and says which
//! it is; a board that does not is either not ours or not running, and those are the two
//! cases a flasher must tell apart before it writes anything.

use std::path::Path;
use std::time::{Duration, Instant};

use serial2::SerialPort;

/// The host baud rate every retinue image's text probe answers at.
pub const PROBE_BAUD: u32 = 115_200;

/// How long to wait for a banner before calling a port silent.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Which board is on a port, as the board itself reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Board {
    /// Heltec Mesh Node T114 (nRF52840). Flashed over serial DFU.
    T114,
    /// Heltec WiFi LoRa 32 V4 (ESP32-S3). Flashed over the ESP ROM loader.
    HeltecV4,
    /// A retinue board whose banner names something this build does not know. Reported
    /// rather than guessed at: flashing the wrong image is the one unrecoverable mistake
    /// this tool can make.
    Unknown(String),
}

impl Board {
    /// Read a board's own banner.
    pub fn from_banner(banner: &str) -> Option<Self> {
        let line = banner.lines().next()?.trim();
        if !line.contains("phy online") && !line.contains("phy ") {
            return None;
        }
        if line.contains("t114") {
            return Some(Board::T114);
        }
        if line.contains("heltec-v4") {
            return Some(Board::HeltecV4);
        }
        Some(Board::Unknown(line.to_string()))
    }

    /// How this board is flashed, in words a person can act on.
    pub fn flash_route(&self) -> &'static str {
        match self {
            Board::T114 => "serial DFU (bootloader probe, then adafruit-nrfutil)",
            Board::HeltecV4 => "ESP ROM loader (espflash)",
            Board::Unknown(_) => "unknown",
        }
    }
}

/// What a port turned out to be.
#[derive(Clone, Debug)]
pub struct Found {
    pub port: String,
    /// `None` when the port answered nothing: not ours, busy, or not running.
    pub board: Option<Board>,
    /// The banner verbatim, for a person to read when the board is unknown.
    pub banner: String,
    /// The regulatory region, asked for rather than read off the banner.
    ///
    /// Only the T114 puts it on its banner; the V4 answers the `region` probe and says
    /// nothing about it unprompted. Asking both makes one listing that means the same thing
    /// whichever board is on the port, which is the point of a survey.
    pub region: Option<String>,
    /// The persisted channel, likewise asked for. No board volunteers it.
    pub channel: Option<String>,
}

impl Found {
    /// The line `linkboy list` prints.
    pub fn describe(&self) -> String {
        match &self.board {
            Some(Board::Unknown(line)) => {
                format!(
                    "{}: a retinue board this build does not know — {line}",
                    self.port
                )
            }
            Some(board) => {
                let mut line = format!("{}: {board:?}", self.port);
                if let Some(region) = &self.region {
                    line.push_str(&format!(" region={region}"));
                }
                if let Some(channel) = &self.channel {
                    line.push_str(&format!(" channel={channel}"));
                }
                line
            }
            None => format!(
                "{}: silent (not a running retinue board, or busy)",
                self.port
            ),
        }
    }
}

/// Pull a `key=value` field out of a banner.
pub fn field(banner: &str, key: &str) -> Option<String> {
    let start = banner.find(key)? + key.len();
    let rest = &banner[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ';')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no serial ports found")]
    NoPorts,
    #[error("{0} did not answer as a retinue board")]
    NotOurs(String),
    #[error("{0} is a retinue board this build does not know how to flash")]
    UnknownBoard(String),
    #[error("the bootloader did not appear within {0:?}")]
    NoBootloader(Duration),
    #[error("{tool} is not installed, and {board:?} needs it")]
    MissingTool { tool: &'static str, board: Board },
    #[error("{tool} failed: {message}")]
    ToolFailed { tool: &'static str, message: String },
    #[error("the image {0} does not exist")]
    NoImage(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Every serial port this machine has.
pub fn ports() -> Result<Vec<String>, Error> {
    let ports = SerialPort::available_ports()?;
    if ports.is_empty() {
        return Err(Error::NoPorts);
    }
    Ok(ports
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

/// Ask one port what it is, and what it is set to.
///
/// A busy port (something else already holds it) reports as silent rather than as an error,
/// because from a person's point of view those are the same situation: not available to
/// flash right now.
///
/// One serial session carries all three questions. Opening the port is the slow part —
/// nearly a second of settling before a board will answer — so asking once and three times
/// over is what keeps a survey of a bench quick.
pub fn identify(port: &str) -> Found {
    // Asked twice before believing silence. The ESP32-S3's USB-serial-JTAG needs a moment to
    // settle after a previous host lets go, and a board surveyed immediately after something
    // else closed the port answers nothing the first time. Silence here means "will not
    // flash this", so a false silence is a false refusal — cheaper to ask again than to send
    // somebody looking for a fault that is not there.
    let mut answers = converse(port, &["status", "region", "channel"]).unwrap_or_default();
    if Board::from_banner(answers.first().map(String::as_str).unwrap_or_default()).is_none() {
        std::thread::sleep(Duration::from_millis(400));
        answers = converse(port, &["status", "region", "channel"]).unwrap_or_default();
    }
    let banner = answers.first().cloned().unwrap_or_default();
    let board = Board::from_banner(&banner);
    // A board with no identity answers "region unavailable: no identity" rather than a
    // region; reporting that verbatim is more use than reporting nothing.
    let region = answers
        .get(1)
        .and_then(|answer| field(answer, "region=").or_else(|| terse(answer)));
    let channel = answers
        .get(2)
        .and_then(|answer| field(answer, "channel=").or_else(|| terse(answer)));
    Found {
        port: port.to_string(),
        board,
        banner,
        region,
        channel,
    }
}

/// A short answer that is not a `key=value`, for reporting refusals as themselves.
fn terse(answer: &str) -> Option<String> {
    let line = answer.lines().find(|line| !line.trim().is_empty())?.trim();
    (line.len() < 60 && !line.contains("phy online")).then(|| line.to_string())
}

/// Send one text probe and collect what comes back.
pub fn probe(port: &str, line: &str) -> Result<String, Error> {
    Ok(converse(port, &[line])?.pop().unwrap_or_default())
}

/// Ask several questions in one serial session, in order.
pub fn converse(port: &str, lines: &[&str]) -> Result<Vec<String>, Error> {
    let mut serial = SerialPort::open(port, PROBE_BAUD)?;
    serial.set_read_timeout(Duration::from_millis(300))?;
    // The T114's CDC endpoint only speaks to a host that has raised DTR; the V4's does not
    // care. Raising it unconditionally is correct for both.
    let _ = serial.set_dtr(true);
    std::thread::sleep(Duration::from_millis(900));

    let mut buffer = [0_u8; 512];
    // Whatever the board volunteered on attach. The T114 writes a banner here and the V4
    // writes nothing, so this is kept and folded into the first answer rather than
    // discarded: on one board it *is* the answer.
    let mut volunteered = String::new();
    while let Ok(read) = serial.read(&mut buffer) {
        if read == 0 {
            break;
        }
        volunteered.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }

    let mut answers = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        serial.write_all(format!("{line}\n").as_bytes())?;
        serial.flush()?;

        // Read until the board goes quiet, not until the first newline. An answer may be more
        // than one line — the V4 replies to `status` with its banner *and* its identity line —
        // and stopping at the first newline leaves the rest in the buffer to be read as the
        // answer to the next question. That off-by-one reported one board's identity line as
        // its region, which is exactly the confident wrong answer a flasher must not give.
        let deadline = Instant::now() + PROBE_TIMEOUT;
        let mut answer = String::new();
        while Instant::now() < deadline {
            match serial.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => answer.push_str(&String::from_utf8_lossy(&buffer[..read])),
                // A read timeout with something already in hand means the answer is done.
                Err(_) if !answer.is_empty() => break,
                Err(_) => continue,
            }
        }
        if index == 0 {
            answer.insert_str(0, &volunteered);
        }
        answers.push(answer);
    }
    Ok(answers)
}

/// Put a T114 into its serial bootloader and wait for the port it comes back on.
///
/// The board reboots into DFU and re-enumerates as a *different* port, which is the part of
/// this dance that is easy to get wrong by hand: the port you were talking to vanishes, and
/// the one you must flash did not exist a moment ago. Watching the set of ports change is
/// what identifies it, rather than guessing a name.
pub fn enter_bootloader(port: &str, patience: Duration) -> Result<String, Error> {
    let before: std::collections::BTreeSet<String> = ports()?.into_iter().collect();

    // The probe's reply is a courtesy; the reset is the contract. A write error here means
    // the board already went, which is success, not failure.
    let _ = probe(port, "bootloader");

    let deadline = Instant::now() + patience;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(400));
        let now: std::collections::BTreeSet<String> = match ports() {
            Ok(ports) => ports.into_iter().collect(),
            Err(_) => continue,
        };
        if let Some(fresh) = now.difference(&before).next() {
            return Ok(fresh.clone());
        }
    }
    Err(Error::NoBootloader(patience))
}

/// Whether a helper tool is on the path.
///
/// Presence is "the operating system could start it", not "it exited zero". Tools disagree
/// about how to be asked their version — `espflash` takes `--version`, `adafruit-nrfutil`
/// wants a `version` subcommand and errors on the flag — so judging by exit status called an
/// installed tool missing. `--help` is only a way to make the process do something harmless
/// and stop; whether it likes the flag is not the question being asked.
pub fn have_tool(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Run a helper tool, reporting its own words on failure.
pub fn run(tool: &'static str, args: &[&str]) -> Result<String, Error> {
    let output = std::process::Command::new(tool)
        .args(args)
        .output()
        .map_err(|error| Error::ToolFailed {
            tool,
            message: error.to_string(),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Error::ToolFailed {
        tool,
        message: format!("{stdout}{stderr}").trim().to_string(),
    })
}

/// Check an image exists before anything irreversible starts.
pub fn require_image(image: &str) -> Result<(), Error> {
    if Path::new(image).exists() {
        Ok(())
    } else {
        Err(Error::NoImage(image.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T114: &str = "tulle/t114 phy online; sx1262 online; spi=software; irq=poll; \
                        sync=2b reg=24b4; region=US915 freq=906875000 reset=soft crash=0\r\n";
    const V4: &str = "tulle/heltec-v4 phy online; sx1262 online; sync=2b reg=24b4; \
                      longfast=906875000\r\n";

    #[test]
    fn boards_are_read_from_their_own_banners() {
        assert_eq!(Board::from_banner(T114), Some(Board::T114));
        assert_eq!(Board::from_banner(V4), Some(Board::HeltecV4));
    }

    /// A retinue board this build does not know is reported, never guessed at. Flashing the
    /// wrong image is the one unrecoverable mistake this tool can make.
    #[test]
    fn an_unfamiliar_retinue_board_is_named_rather_than_guessed() {
        let banner = "tulle/rak4631 phy online; sx1262 online\r\n";
        match Board::from_banner(banner) {
            Some(Board::Unknown(line)) => assert!(line.contains("rak4631")),
            other => panic!("an unknown board must not be identified as one we know: {other:?}"),
        }
    }

    #[test]
    fn silence_and_strangers_are_not_boards() {
        assert_eq!(Board::from_banner(""), None);
        assert_eq!(Board::from_banner("login: "), None);
        assert_eq!(Board::from_banner("ESP-ROM:esp32s3-20210327"), None);
    }

    #[test]
    fn banner_fields_are_read_for_the_listing() {
        assert_eq!(field(T114, "region=").as_deref(), Some("US915"));
        assert_eq!(field(T114, "freq=").as_deref(), Some("906875000"));
        assert_eq!(field(T114, "reset=").as_deref(), Some("soft"));
        assert_eq!(field(T114, "nothing=").as_deref(), None);
        // A field ending at a semicolon rather than whitespace still reads cleanly.
        assert_eq!(field("a; sync=2b; b", "sync=").as_deref(), Some("2b"));
    }

    #[test]
    fn a_found_board_describes_itself_for_a_person() {
        let found = Found {
            port: "COM10".into(),
            board: Some(Board::T114),
            banner: T114.into(),
            region: Some("US915".into()),
            channel: Some("rnode".into()),
        };
        let line = found.describe();
        assert!(line.contains("COM10"), "{line}");
        assert!(line.contains("T114"), "{line}");
        assert!(line.contains("US915"), "{line}");
        // The channel is the field a bench most often wants and no board volunteers, so a
        // listing that omitted it would send a person back to a second tool.
        assert!(line.contains("rnode"), "{line}");

        let silent = Found {
            port: "COM3".into(),
            board: None,
            banner: String::new(),
            region: None,
            channel: None,
        };
        assert!(silent.describe().contains("silent"));
    }
}
