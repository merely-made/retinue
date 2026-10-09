//! Literal USB Serial/JTAG carrier for signed control frames.

use std::io;
use std::path::Path;
use std::time::Duration;

use seneschal::control::{
    CONTROL_RESPONSE_FRAME_TAG, ControlFrameError, MAX_CONTROL_COMMAND_FRAME_LEN,
    MAX_CONTROL_RESPONSE_FRAME_LEN, Response, decode_response_frame, encode_command_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::ControlExchange;

/// Literal USB Serial/JTAG settings for the signed control carrier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbControlConfig {
    pub baud_rate: u32,
    /// The board journals the accepted counter to flash inside a radio quiet window before it
    /// answers, so this is longer than the diagnostic's timeout.
    pub response_timeout: Duration,
}

impl Default for UsbControlConfig {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            response_timeout: Duration::from_secs(5),
        }
    }
}

impl UsbControlConfig {
    /// The V4's native USB control lines must both remain deasserted.
    pub const fn dtr(&self) -> bool {
        false
    }

    /// The V4's native USB control lines must both remain deasserted.
    pub const fn rts(&self) -> bool {
        false
    }
}

/// Signed-carrier failure. Silence is the board's answer to every outer refusal, so a
/// timeout here means the wrong key, a stale or too-far counter, or a board that could not
/// quiet its radio, and not necessarily an absent board.
#[derive(Debug, thiserror::Error)]
pub enum UsbControlError {
    #[error("control USB I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("control response timed out; the board answers refusals with silence")]
    Timeout,
    #[error("control USB stream ended before a response")]
    Eof,
    #[error("malformed control frame: {0:?}")]
    Malformed(ControlFrameError),
}

/// Reusable raw serial carrier for signed commands. It holds no signer or identity.
pub struct UsbControlTransport<T> {
    io: T,
    config: UsbControlConfig,
    deframer: selvage::kiss::Deframer<MAX_CONTROL_RESPONSE_FRAME_LEN>,
}

impl UsbControlTransport<serial2_tokio::SerialPort> {
    /// Opens one explicit ordinary-runtime serial path with both native USB control lines
    /// deasserted.
    pub fn open(path: impl AsRef<Path>, config: UsbControlConfig) -> Result<Self, UsbControlError> {
        let port =
            serial2_tokio::SerialPort::open(path, config.baud_rate).map_err(UsbControlError::Io)?;
        port.set_dtr(config.dtr()).map_err(UsbControlError::Io)?;
        port.set_rts(config.rts()).map_err(UsbControlError::Io)?;
        Ok(Self::from_io(port, config))
    }
}

impl<T> UsbControlTransport<T> {
    pub fn from_io(io: T, config: UsbControlConfig) -> Self {
        Self {
            io,
            config,
            deframer: selvage::kiss::Deframer::new(),
        }
    }

    pub fn into_io(self) -> T {
        self.io
    }
}

impl<T> ControlExchange for UsbControlTransport<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Error = UsbControlError;

    async fn exchange(&mut self, command: &[u8]) -> Result<Response, Self::Error> {
        let mut frame = [0_u8; MAX_CONTROL_COMMAND_FRAME_LEN];
        let frame_len =
            encode_command_frame(command, &mut frame).map_err(UsbControlError::Malformed)?;
        let mut wire = [0_u8; 2 + MAX_CONTROL_COMMAND_FRAME_LEN * 2];
        let wire_len = selvage::kiss::encode_into(&frame[..frame_len], &mut wire)
            .expect("the fixed command KISS buffer is sufficient");
        self.io
            .write_all(&wire[..wire_len])
            .await
            .map_err(UsbControlError::Io)?;
        self.io.flush().await.map_err(UsbControlError::Io)?;

        tokio::time::timeout(self.config.response_timeout, async {
            let mut bytes = [0_u8; 256];
            loop {
                let read = self
                    .io
                    .read(&mut bytes)
                    .await
                    .map_err(UsbControlError::Io)?;
                if read == 0 {
                    return Err(UsbControlError::Eof);
                }
                for &byte in &bytes[..read] {
                    if !self.deframer.push(byte) {
                        continue;
                    }
                    let frame = self.deframer.frame();
                    if frame.first() != Some(&CONTROL_RESPONSE_FRAME_TAG) {
                        continue;
                    }
                    return decode_response_frame(frame).map_err(UsbControlError::Malformed);
                }
            }
        })
        .await
        .unwrap_or(Err(UsbControlError::Timeout))
    }
}
