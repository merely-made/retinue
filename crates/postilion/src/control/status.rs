//! Normal-runtime, diagnostic-only control-status transport.
//!
//! This is deliberately separate from the physical first-owner carrier. It
//! sends only a bounded KISS request to an already-running modem and returns
//! public evidence marked `diagnostic-only`; it has no signer, claim flow, or
//! durable-write capability.

use std::io;
use std::path::Path;
use std::time::Duration;

use radio_hand::control::{
    CONTROL_STATUS_FRAME_LEN, CONTROL_STATUS_FRAME_TAG, CONTROL_STATUS_NONCE_LEN,
    CONTROL_STATUS_REQUEST_FRAME_LEN, ControlStatusAuthority, ControlStatusError,
    ControlStatusEvidence, ControlStatusRequestV1, ControlStatusV1, NodeId,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Literal USB Serial/JTAG settings for one ordinary-runtime status read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbControlStatusConfig {
    pub baud_rate: u32,
    pub response_timeout: Duration,
}

impl Default for UsbControlStatusConfig {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            response_timeout: Duration::from_secs(3),
        }
    }
}

impl UsbControlStatusConfig {
    pub const fn dtr(&self) -> bool {
        false
    }

    pub const fn rts(&self) -> bool {
        false
    }
}

/// Status-carrier failure. None of these outcomes authenticate a board or a
/// host: this carrier transports only public diagnostic evidence.
#[derive(Debug, thiserror::Error)]
pub enum UsbControlStatusError {
    #[error("control-status USB I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("control-status response timed out")]
    Timeout,
    #[error("control-status USB stream ended before a response")]
    Eof,
    #[error("malformed control-status response: {0:?}")]
    Malformed(ControlStatusError),
    #[error("control-status response nonce did not match this query")]
    NonceMismatch,
}

fn validate_response_nonce(
    status: ControlStatusV1,
    nonce: [u8; CONTROL_STATUS_NONCE_LEN],
) -> Result<ControlStatusV1, UsbControlStatusError> {
    if status.query_nonce() == nonce {
        Ok(status)
    } else {
        Err(UsbControlStatusError::NonceMismatch)
    }
}

/// Consumer-side refusal to promote a public diagnostic to an authenticated
/// status or to accept it for a different expected node.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ControlStatusValidationError {
    #[error("control-status authority was not diagnostic-only")]
    Authority,
    #[error("control-status node did not match the expected node")]
    NodeMismatch { expected: NodeId, found: NodeId },
    #[error("control-status did not show valid ordinary control evidence")]
    ControlEvidence(ControlStatusEvidence),
    #[error("control-status pending evidence was not blank")]
    PendingEvidence(ControlStatusEvidence),
    #[error("control-status boot fact was not known-good-applied")]
    BootFact,
}

/// Validates only the public evidence a bench needs before displaying it.
pub fn validate_diagnostic_status(
    status: ControlStatusV1,
    expected_node: NodeId,
) -> Result<ControlStatusV1, ControlStatusValidationError> {
    if status.authority() != ControlStatusAuthority::DiagnosticOnly {
        return Err(ControlStatusValidationError::Authority);
    }
    if status.node() != expected_node {
        return Err(ControlStatusValidationError::NodeMismatch {
            expected: expected_node,
            found: status.node(),
        });
    }
    if status.control() != ControlStatusEvidence::Valid {
        return Err(ControlStatusValidationError::ControlEvidence(
            status.control(),
        ));
    }
    if status.pending() != ControlStatusEvidence::Blank {
        return Err(ControlStatusValidationError::PendingEvidence(
            status.pending(),
        ));
    }
    if status.boot() != radio_hand::control::ControlStatusBootFact::KnownGoodApplied {
        return Err(ControlStatusValidationError::BootFact);
    }
    Ok(status)
}

/// Reusable, raw serial diagnostic carrier. It holds neither a signer nor an
/// identity and never touches the first-owner USB session.
pub struct UsbControlStatusTransport<T> {
    io: T,
    config: UsbControlStatusConfig,
    deframer: selvage::kiss::Deframer<CONTROL_STATUS_FRAME_LEN>,
}

impl UsbControlStatusTransport<serial2_tokio::SerialPort> {
    /// Opens one explicit ordinary-runtime serial path with both native USB
    /// control lines deasserted.
    pub fn open(
        path: impl AsRef<Path>,
        config: UsbControlStatusConfig,
    ) -> Result<Self, UsbControlStatusError> {
        let port = serial2_tokio::SerialPort::open(path, config.baud_rate)
            .map_err(UsbControlStatusError::Io)?;
        port.set_dtr(config.dtr())
            .map_err(UsbControlStatusError::Io)?;
        port.set_rts(config.rts())
            .map_err(UsbControlStatusError::Io)?;
        Ok(Self::from_io(port, config))
    }
}

impl<T> UsbControlStatusTransport<T> {
    pub fn from_io(io: T, config: UsbControlStatusConfig) -> Self {
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

impl<T> UsbControlStatusTransport<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    /// Requests one status snapshot. Reads are deliberately deframed byte by
    /// byte across arbitrary USB chunks; unrelated plain modem events and
    /// foreign KISS frames are ignored until the tagged exact response arrives.
    pub async fn read(&mut self) -> Result<ControlStatusV1, UsbControlStatusError> {
        let mut nonce = [0; CONTROL_STATUS_NONCE_LEN];
        getrandom::fill(&mut nonce)
            .map_err(|error| UsbControlStatusError::Io(io::Error::other(error.to_string())))?;
        self.read_with_nonce(nonce).await
    }

    pub async fn read_with_nonce(
        &mut self,
        nonce: [u8; CONTROL_STATUS_NONCE_LEN],
    ) -> Result<ControlStatusV1, UsbControlStatusError> {
        let mut request = [0_u8; CONTROL_STATUS_REQUEST_FRAME_LEN];
        ControlStatusRequestV1::new(nonce)
            .encode_frame(&mut request)
            .expect("fixed status request buffer matches its portable contract");
        let mut wire = [0_u8; 2 + CONTROL_STATUS_REQUEST_FRAME_LEN * 2];
        let wire_len = selvage::kiss::encode_into(&request, &mut wire)
            .expect("fixed status KISS request buffer is sufficient");
        self.io
            .write_all(&wire[..wire_len])
            .await
            .map_err(UsbControlStatusError::Io)?;
        self.io.flush().await.map_err(UsbControlStatusError::Io)?;

        tokio::time::timeout(self.config.response_timeout, async {
            let mut bytes = [0_u8; 256];
            loop {
                let read = self
                    .io
                    .read(&mut bytes)
                    .await
                    .map_err(UsbControlStatusError::Io)?;
                if read == 0 {
                    return Err(UsbControlStatusError::Eof);
                }
                for &byte in &bytes[..read] {
                    if !self.deframer.push(byte) {
                        continue;
                    }
                    let frame = self.deframer.frame();
                    if frame.first() != Some(&CONTROL_STATUS_FRAME_TAG) {
                        continue;
                    }
                    let status = ControlStatusV1::decode_frame(frame)
                        .map_err(UsbControlStatusError::Malformed)?;
                    return validate_response_nonce(status, nonce);
                }
            }
        })
        .await
        .unwrap_or(Err(UsbControlStatusError::Timeout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(node: NodeId) -> ControlStatusV1 {
        let mut bytes = [0_u8; radio_hand::control::CONTROL_STATUS_V1_LEN];
        bytes[0] = radio_hand::control::CONTROL_STATUS_VERSION;
        bytes[1] = 0;
        bytes[2] = 0;
        bytes[3] = 1;
        bytes[4] = 0;
        bytes[5..21].copy_from_slice(&node.0);
        bytes[21..29].copy_from_slice(&7_u64.to_le_bytes());
        bytes[29..37].copy_from_slice(&9_u64.to_le_bytes());
        ControlStatusV1::decode(&bytes).unwrap()
    }

    #[tokio::test]
    async fn fragmented_response_resynchronizes_past_normal_events() {
        let node = NodeId([0x71; 16]);
        let snapshot = status(node);
        let (client, mut board) = tokio::io::duplex(256);
        let board_task = tokio::spawn(async move {
            let mut request = [0_u8; 64];
            let read = board.read(&mut request).await.unwrap();
            let mut parsed = selvage::kiss::Deframer::<CONTROL_STATUS_REQUEST_FRAME_LEN>::new();
            let mut seen = false;
            for byte in request[..read].iter().copied() {
                if parsed.push(byte) && ControlStatusRequestV1::decode_frame(parsed.frame()).is_ok()
                {
                    seen = true;
                }
            }
            assert!(seen);

            let mut frame = [0_u8; CONTROL_STATUS_FRAME_LEN];
            snapshot.encode_frame(&mut frame).unwrap();
            let mut wire = [0_u8; 2 + CONTROL_STATUS_FRAME_LEN * 2];
            let len = selvage::kiss::encode_into(&frame, &mut wire).unwrap();
            board.write_all(b"normal event\r\n").await.unwrap();
            board.write_all(&wire[..3]).await.unwrap();
            board.write_all(&wire[3..len]).await.unwrap();
        });

        let mut transport = UsbControlStatusTransport::from_io(
            client,
            UsbControlStatusConfig {
                response_timeout: Duration::from_secs(1),
                ..UsbControlStatusConfig::default()
            },
        );
        let received = transport
            .read_with_nonce([0; CONTROL_STATUS_NONCE_LEN])
            .await
            .unwrap();
        board_task.await.unwrap();
        assert_eq!(received, snapshot);
        assert_eq!(validate_diagnostic_status(received, node), Ok(snapshot));
        assert!(matches!(
            validate_diagnostic_status(received, NodeId([0; 16])),
            Err(ControlStatusValidationError::NodeMismatch { .. })
        ));
    }

    #[test]
    fn stale_nonce_is_refused_before_diagnostic_validation() {
        let status = status(NodeId([0x72; 16])).with_query_nonce([1; CONTROL_STATUS_NONCE_LEN]);
        assert!(matches!(
            validate_response_nonce(status, [2; CONTROL_STATUS_NONCE_LEN]),
            Err(UsbControlStatusError::NonceMismatch)
        ));
    }
}
