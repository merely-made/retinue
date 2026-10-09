//! Literal USB Serial/JTAG carrier with KISS framing.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use seneschal::control::{
    CLAIM_REQUEST_LEN, FirstOwnerRequest, FirstOwnerResponse, FirstOwnerWireError,
    INSPECT_RESPONSE_LEN,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tulle::kiss;

use super::FirstOwnerExchange;

/// Literal USB Serial/JTAG connection settings for the V4 first-owner carrier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbFirstOwnerConfig {
    pub baud_rate: u32,
    pub response_timeout: Duration,
    pub session_timeout: Duration,
}

impl Default for UsbFirstOwnerConfig {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            response_timeout: Duration::from_secs(2),
            session_timeout: Duration::from_secs(45),
        }
    }
}

impl UsbFirstOwnerConfig {
    /// The V4's native USB control lines must both remain deasserted.
    pub const fn dtr(&self) -> bool {
        false
    }

    /// The V4's native USB control lines must both remain deasserted.
    pub const fn rts(&self) -> bool {
        false
    }
}

/// Serial/KISS carrier error. An EOF is not a terminal-operation acknowledgement.
#[derive(Debug, thiserror::Error)]
pub enum UsbFirstOwnerError {
    #[error("first-owner USB I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("first-owner board session expired")]
    SessionExpired,
    #[error("first-owner response timed out")]
    Timeout,
    #[error("first-owner USB stream ended before a response")]
    Eof,
    #[error("malformed first-owner response: {0:?}")]
    Malformed(FirstOwnerWireError),
    #[error("first-owner response did not match the request")]
    MismatchedResponse,
    #[error("first-owner carrier must be reopened and freshly inspected after an exchange error")]
    ReconnectRequired,
}

/// Reusable raw USB carrier. It deliberately has no configuration or identity field.
pub struct UsbFirstOwnerTransport<T> {
    io: T,
    config: UsbFirstOwnerConfig,
    opened_at: Instant,
    deframer: kiss::Deframer,
    poisoned: bool,
}

impl UsbFirstOwnerTransport<serial2_tokio::SerialPort> {
    /// Opens one explicit serial path. DTR and RTS remain false so native USB control-line
    /// transitions cannot reset a V4 that is in its physical-presence window.
    pub fn open(
        path: impl AsRef<Path>,
        config: UsbFirstOwnerConfig,
    ) -> Result<Self, UsbFirstOwnerError> {
        let port = serial2_tokio::SerialPort::open(path, config.baud_rate)
            .map_err(UsbFirstOwnerError::Io)?;
        port.set_dtr(config.dtr()).map_err(UsbFirstOwnerError::Io)?;
        port.set_rts(config.rts()).map_err(UsbFirstOwnerError::Io)?;
        Ok(Self::from_io(port, config))
    }
}

impl<T> UsbFirstOwnerTransport<T> {
    pub fn from_io(io: T, config: UsbFirstOwnerConfig) -> Self {
        Self {
            io,
            config,
            opened_at: Instant::now(),
            deframer: kiss::Deframer::new(INSPECT_RESPONSE_LEN),
            poisoned: false,
        }
    }

    pub fn into_io(self) -> T {
        self.io
    }
}

impl<T> UsbFirstOwnerTransport<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    async fn exchange_inner(
        &mut self,
        request: FirstOwnerRequest,
    ) -> Result<FirstOwnerResponse, UsbFirstOwnerError> {
        let elapsed = self.opened_at.elapsed();
        let remaining = self
            .config
            .session_timeout
            .checked_sub(elapsed)
            .ok_or(UsbFirstOwnerError::SessionExpired)?;
        let request_kind = request_kind(&request);
        let mut raw = [0_u8; CLAIM_REQUEST_LEN];
        let length = request
            .encode(&mut raw[..request_len(&request)])
            .expect("request length comes from its portable exact contract");
        self.io
            .write_all(&kiss::encode(&raw[..length]))
            .await
            .map_err(UsbFirstOwnerError::Io)?;
        self.io.flush().await.map_err(UsbFirstOwnerError::Io)?;

        let timeout = self.config.response_timeout.min(remaining);
        tokio::time::timeout(timeout, async {
            let mut bytes = [0_u8; 256];
            loop {
                let read = self
                    .io
                    .read(&mut bytes)
                    .await
                    .map_err(UsbFirstOwnerError::Io)?;
                if read == 0 {
                    return Err(UsbFirstOwnerError::Eof);
                }
                let mut frames = Vec::new();
                self.deframer.push(&bytes[..read], &mut frames);
                if let Some(frame) = frames.into_iter().next() {
                    let response = FirstOwnerResponse::decode(&frame)
                        .map_err(UsbFirstOwnerError::Malformed)?;
                    if response_kind(&response) != request_kind {
                        return Err(UsbFirstOwnerError::MismatchedResponse);
                    }
                    return Ok(response);
                }
            }
        })
        .await
        .unwrap_or(Err(UsbFirstOwnerError::Timeout))
    }
}

impl<T> FirstOwnerExchange for UsbFirstOwnerTransport<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Error = UsbFirstOwnerError;

    async fn exchange(
        &mut self,
        request: FirstOwnerRequest,
    ) -> Result<FirstOwnerResponse, Self::Error> {
        if self.poisoned {
            return Err(UsbFirstOwnerError::ReconnectRequired);
        }
        let result = self.exchange_inner(request).await;
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

fn request_len(request: &FirstOwnerRequest) -> usize {
    match request {
        FirstOwnerRequest::Claim(_) => CLAIM_REQUEST_LEN,
        FirstOwnerRequest::Inspect | FirstOwnerRequest::Resume | FirstOwnerRequest::Abandon => 2,
    }
}

fn request_kind(request: &FirstOwnerRequest) -> u8 {
    match request {
        FirstOwnerRequest::Inspect => 1,
        FirstOwnerRequest::Claim(_) => 2,
        FirstOwnerRequest::Resume => 3,
        FirstOwnerRequest::Abandon => 4,
    }
}

fn response_kind(response: &FirstOwnerResponse) -> u8 {
    match response {
        FirstOwnerResponse::Inspect { .. } => 1,
        FirstOwnerResponse::Claim(_) => 2,
        FirstOwnerResponse::Resume(_) => 3,
        FirstOwnerResponse::Abandon(_) => 4,
    }
}
