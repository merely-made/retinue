//! Opening an RNode's serial line: serial2 for a real port, and a fallback for a pty.
//!
//! macOS sets the line speed with an ioctl a pty refuses with ENOTTY, so serial2 cannot open
//! one. Such a line is used as its other end configured it, which is what a pty pair wants.

use std::io;
use std::path::Path;

use serial2_tokio::SerialPort;
use tokio::io::{AsyncRead, AsyncWrite};

/// An open line.
pub(super) trait Port: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Port for T {}

const EINVAL: i32 = 22;
const ENOTTY: i32 = 25;

pub(super) fn open(path: &Path, baud: u32) -> io::Result<Box<dyn Port>> {
    let port = match SerialPort::open(path, baud) {
        Ok(port) => port,
        Err(error) if error.raw_os_error() == Some(ENOTTY) => return open_unconfigured(path),
        Err(error) => return Err(error),
    };
    // nRF USB CDC gates output on DTR. On ESP32, RTS is wired into reset/boot and must
    // remain deasserted (asserting it wedged a board in a live harness). A line without
    // modem control, as a pty is, is tolerated as pyserial tolerates it.
    for line in [port.set_dtr(true), port.set_rts(false)] {
        line.or_else(|error| match error.raw_os_error() {
            Some(ENOTTY | EINVAL) => Ok(()),
            _ => Err(error),
        })?;
    }
    Ok(Box::new(port))
}

#[cfg(target_os = "macos")]
fn open_unconfigured(path: &Path) -> io::Result<Box<dyn Port>> {
    use std::os::unix::fs::OpenOptionsExt;
    use tokio::net::unix::pipe;
    // <sys/fcntl.h> on Darwin.
    const O_NONBLOCK: i32 = 0x0004;
    const O_NOCTTY: i32 = 0x0002_0000;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK | O_NOCTTY)
        .open(path)?;
    let writer = pipe::Sender::from_file_unchecked(file.try_clone()?)?;
    let reader = pipe::Receiver::from_file_unchecked(file)?;
    Ok(Box::new(tokio::io::join(reader, writer)))
}

#[cfg(not(target_os = "macos"))]
fn open_unconfigured(_: &Path) -> io::Result<Box<dyn Port>> {
    Err(io::Error::from_raw_os_error(ENOTTY))
}
