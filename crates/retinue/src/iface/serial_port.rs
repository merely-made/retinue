//! Opening a serial line: serial2 for a real port, with RNS's line settings, and a fallback
//! for a pty.
//!
//! macOS sets the line speed with an ioctl a pty refuses with ENOTTY, so serial2 cannot open
//! one. Such a line is used as its other end configured it, which is what a pty pair wants.

use alloc::boxed::Box;

use std::io;

use serial2_tokio::{CharSize, FlowControl, SerialPort, Settings, StopBits};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{Parity, SerialConfig};

/// An open line.
pub(super) trait Port: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Port for T {}

const EINVAL: i32 = 22;
const ENOTTY: i32 = 25;

/// Open `config`'s line with best-effort modem lines.
pub(super) fn open(config: &SerialConfig) -> io::Result<Box<dyn Port>> {
    let port = match SerialPort::open(&config.path, |settings| config.apply(settings)) {
        Ok(port) => port,
        Err(error) if error.raw_os_error() == Some(ENOTTY) => return open_unconfigured(config),
        Err(error) => return Err(error),
    };
    // pyserial asserts DTR and RTS but tolerates a line that has neither, as a pty has not.
    for line in [port.set_dtr(true), port.set_rts(true)] {
        line.or_else(|error| match error.raw_os_error() {
            Some(ENOTTY | EINVAL) => Ok(()),
            _ => Err(error),
        })?;
    }
    Ok(Box::new(port))
}

impl SerialConfig {
    pub(super) fn char_size(&self) -> io::Result<CharSize> {
        Ok(match self.databits {
            5 => CharSize::Bits5,
            6 => CharSize::Bits6,
            7 => CharSize::Bits7,
            8 => CharSize::Bits8,
            _ => return Err(invalid("databits must be 5 to 8")),
        })
    }

    pub(super) fn stop_bits(&self) -> io::Result<StopBits> {
        Ok(match self.stopbits {
            1 => StopBits::One,
            2 => StopBits::Two,
            _ => return Err(invalid("stopbits must be 1 or 2")),
        })
    }

    fn apply(&self, mut settings: Settings) -> io::Result<Settings> {
        settings.set_raw();
        settings.set_baud_rate(self.speed)?;
        settings.set_char_size(self.char_size()?);
        settings.set_stop_bits(self.stop_bits()?);
        settings.set_parity(match self.parity {
            Parity::None => serial2_tokio::Parity::None,
            Parity::Even => serial2_tokio::Parity::Even,
            Parity::Odd => serial2_tokio::Parity::Odd,
        });
        settings.set_flow_control(FlowControl::None);
        Ok(settings)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(target_os = "macos")]
fn open_unconfigured(config: &SerialConfig) -> io::Result<Box<dyn Port>> {
    use std::os::unix::fs::OpenOptionsExt;
    use tokio::net::unix::pipe;
    // <sys/fcntl.h> on Darwin.
    const O_NONBLOCK: i32 = 0x0004;
    const O_NOCTTY: i32 = 0x0002_0000;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK | O_NOCTTY)
        .open(&config.path)?;
    let writer = pipe::Sender::from_file_unchecked(file.try_clone()?)?;
    let reader = pipe::Receiver::from_file_unchecked(file)?;
    Ok(Box::new(tokio::io::join(reader, writer)))
}

#[cfg(not(target_os = "macos"))]
fn open_unconfigured(_: &SerialConfig) -> io::Result<Box<dyn Port>> {
    Err(io::Error::from_raw_os_error(ENOTTY))
}
