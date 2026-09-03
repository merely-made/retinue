//! Normal-runtime, controller-authenticated control over a local byte-stream carrier.
//!
//! This is the host half of the V4's signed control carrier. The controller signs one WN0
//! request with `retinue::command`, the carrier frames it for the board's ordinary modem
//! stream, and the board answers only after its verifier has accepted the envelope and its
//! runtime has journaled the accepted counter. The response is not signed by the board:
//! what this module checks is that the answer names the node and transaction it asked
//! about and carries `VerifiedController` authority, which the board produces for nothing
//! else.
//!
//! The outer counter is the controller's to remember. The board refuses a counter at or
//! below its last accepted value and one more than `COUNTER_WINDOW` ahead, so the
//! application persists what it last used; Postilion never stores it.

use std::future::Future;
use std::io;
use std::path::Path;
use std::time::Duration;

use radio_hand::control::{
    CONTROL_RESPONSE_FRAME_TAG, ConfigGeneration, ControlFrameError, ControlStatusAuthority,
    ControlStatusError, ControlStatusV1, MAX_CONTROL_COMMAND_FRAME_LEN,
    MAX_CONTROL_RESPONSE_FRAME_LEN, NodeId, Operation, Refusal, Request, Response, ResponseBody,
    TransactionId, decode_response_frame, encode_command_frame,
};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::sign_request;

/// Carrier-neutral exchange of one signed outer command for one WN0 response.
///
/// Implementations receive only the signed wire bytes. The signer stays with the
/// [`ControlClient`] that built them.
pub trait ControlExchange {
    type Error;

    fn exchange(&mut self, command: &[u8]) -> impl Future<Output = Result<Response, Self::Error>>;
}

/// Why a signed control exchange did not yield a usable answer.
#[derive(Debug, thiserror::Error)]
pub enum ControlClientError<E> {
    #[error("control carrier failure")]
    Carrier(E),
    #[error("control request could not be signed: {0}")]
    Signing(#[source] super::Error),
    #[error("controller entropy unavailable: {0}")]
    Entropy(#[source] io::Error),
    #[error("control response named node {found:?}, expected {expected:?}")]
    NodeMismatch { expected: NodeId, found: NodeId },
    #[error("control response belongs to a different transaction")]
    TransactionMismatch,
    #[error("node refused the request: {0:?}")]
    Refused(Refusal),
    #[error("node answered with an unexpected body")]
    UnexpectedBody,
    #[error("malformed verified status body: {0:?}")]
    MalformedStatus(ControlStatusError),
    #[error("status body did not carry verified-controller authority")]
    Authority,
    #[error("status body was not bound to this transaction")]
    StatusTransactionMismatch,
}

/// A verified controller's view of one node's public control status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedStatus {
    pub transaction: TransactionId,
    pub counter: u64,
    pub known_good_generation: ConfigGeneration,
    pub status: ControlStatusV1,
}

/// The carrier-neutral controller. It owns the signer borrow and the node it addresses.
pub struct ControlClient<'a, C> {
    carrier: C,
    signer: &'a PrivateIdentity,
    node: NodeId,
}

impl<'a, C> ControlClient<'a, C> {
    pub fn new(carrier: C, signer: &'a PrivateIdentity, node: NodeId) -> Self {
        Self {
            carrier,
            signer,
            node,
        }
    }

    pub fn into_carrier(self) -> C {
        self.carrier
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }
}

impl<C> ControlClient<'_, C>
where
    C: ControlExchange,
{
    /// Signs and sends one `Status` request under `counter`, and returns the node's
    /// verified-controller status once the answer has been bound back to this request.
    pub async fn status(
        &mut self,
        counter: u64,
    ) -> Result<VerifiedStatus, ControlClientError<C::Error>> {
        let mut transaction = [0_u8; 16];
        getrandom::fill(&mut transaction)
            .map_err(|error| ControlClientError::Entropy(io::Error::other(error.to_string())))?;
        let request = Request {
            transaction: TransactionId(transaction),
            transaction_sequence: 0,
            expected_generation: ConfigGeneration(0),
            operation: Operation::Status,
            arguments: heapless::Vec::new(),
        };
        let response = self.send(&request, counter).await?;
        let ResponseBody::Observed(body) = &response.body else {
            return Err(ControlClientError::UnexpectedBody);
        };
        let status = ControlStatusV1::decode(body).map_err(ControlClientError::MalformedStatus)?;
        if status.authority() != ControlStatusAuthority::VerifiedController {
            return Err(ControlClientError::Authority);
        }
        if status.query_nonce() != transaction || status.node() != self.node {
            return Err(ControlClientError::StatusTransactionMismatch);
        }
        Ok(VerifiedStatus {
            transaction: request.transaction,
            counter,
            known_good_generation: response.known_good_generation,
            status,
        })
    }

    async fn send(
        &mut self,
        request: &Request,
        counter: u64,
    ) -> Result<Response, ControlClientError<C::Error>> {
        let wire = sign_request(
            request,
            self.signer,
            AddressHash::from_bytes(self.node.0),
            counter,
        )
        .map_err(ControlClientError::Signing)?;
        let response = self
            .carrier
            .exchange(&wire)
            .await
            .map_err(ControlClientError::Carrier)?;
        if response.node != self.node {
            return Err(ControlClientError::NodeMismatch {
                expected: self.node,
                found: response.node,
            });
        }
        if response.transaction != request.transaction {
            return Err(ControlClientError::TransactionMismatch);
        }
        if let ResponseBody::Refused { reason, .. } = &response.body {
            return Err(ControlClientError::Refused(*reason));
        }
        Ok(response)
    }
}

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

    fn exchange(&mut self, command: &[u8]) -> impl Future<Output = Result<Response, Self::Error>> {
        async move {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use heapless::Vec;
    use radio_hand::control::{
        BoardRecoveryFacts, ControllerRole, DurableConfig, DurableState, FirstWriteStatus,
        ManagementCarrier, ManagementCarrierSet, OwnerGrant, PairEvidence, PublicConfigurationV1,
        RecoveryClause, RecoveryPathFacts, RecoveryPolicy, ReticulumTransportPolicy,
        decode_command_frame, decode_verified_command, encode_response_frame,
        restore_control_verifier,
    };
    use radio_hand::region::Region;

    const NODE: NodeId = NodeId([0x5a; 16]);

    fn owner() -> PrivateIdentity {
        PrivateIdentity::from_secret_bytes(&[0x33; 64])
    }

    fn state(owner: &PrivateIdentity) -> DurableState {
        let public = PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap();
        let policy = RecoveryPolicy::new(
            RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
            RecoveryClause::disabled(),
        )
        .unwrap();
        let facts = BoardRecoveryFacts::new(
            Vec::from_slice(&[
                RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        DurableState::new(
            NODE,
            Vec::from_slice(&[OwnerGrant::from_retinue_identity(
                owner.public(),
                ControllerRole::Owner,
            )])
            .unwrap(),
            ConfigGeneration(3),
            DurableConfig {
                public,
                sealed_credentials: Vec::new(),
            },
            policy,
            &facts,
        )
        .unwrap()
    }

    /// What a board does with one frame: verify with a grant-restored verifier, journal the
    /// counter (modelled by the state mutation), and answer the Status it decoded.
    fn board_answer(
        state: &mut DurableState,
        frame: &[u8],
        authority_diagnostic: bool,
    ) -> Response {
        let mut verifier = restore_control_verifier(state).unwrap();
        let command = decode_command_frame(frame).unwrap();
        let verified = verifier.verify(command).unwrap();
        let inbound = decode_verified_command(&verified).unwrap();
        state
            .advance_verified_outer_counter(inbound.verified_controller(), inbound.counter())
            .unwrap();
        let first_write = FirstWriteStatus {
            control: PairEvidence::Valid,
            pending: PairEvidence::Blank,
        };
        let status = if authority_diagnostic {
            ControlStatusV1::from_recovered_state(first_write, state, false)
                .with_query_nonce(inbound.request().transaction.0)
        } else {
            ControlStatusV1::for_verified_controller(
                first_write,
                state,
                false,
                inbound.request().transaction,
            )
        };
        let mut bytes = [0_u8; radio_hand::control::CONTROL_STATUS_V1_LEN];
        status.encode(&mut bytes).unwrap();
        Response {
            node: NODE,
            transaction: inbound.request().transaction,
            known_good_generation: state.known_good().generation,
            effective_generation: None,
            body: ResponseBody::Observed(Vec::from_slice(&bytes).unwrap()),
        }
    }

    async fn read_one_frame<R: AsyncRead + Unpin>(board: &mut R) -> std::vec::Vec<u8> {
        let mut deframer = selvage::kiss::Deframer::<MAX_CONTROL_COMMAND_FRAME_LEN>::new();
        let mut byte = [0_u8; 1];
        loop {
            board.read_exact(&mut byte).await.unwrap();
            if deframer.push(byte[0]) {
                return deframer.frame().to_vec();
            }
        }
    }

    async fn write_response<W: AsyncWrite + Unpin>(board: &mut W, response: &Response) {
        let mut frame = [0_u8; MAX_CONTROL_RESPONSE_FRAME_LEN];
        let len = encode_response_frame(response, &mut frame).unwrap();
        let mut wire = [0_u8; 2 + MAX_CONTROL_RESPONSE_FRAME_LEN * 2];
        let wire_len = selvage::kiss::encode_into(&frame[..len], &mut wire).unwrap();
        board.write_all(b"ordinary modem event\r\n").await.unwrap();
        board.write_all(&wire[..3]).await.unwrap();
        board.write_all(&wire[3..wire_len]).await.unwrap();
    }

    #[tokio::test]
    async fn signed_status_is_verified_journaled_and_bound_back_to_its_transaction() {
        let owner = owner();
        let mut state = state(&owner);
        let (client, mut board) = tokio::io::duplex(2048);
        let board_task = tokio::spawn(async move {
            let frame = read_one_frame(&mut board).await;
            let response = board_answer(&mut state, &frame, false);
            write_response(&mut board, &response).await;
            state
        });

        let transport = UsbControlTransport::from_io(
            client,
            UsbControlConfig {
                response_timeout: Duration::from_secs(2),
                ..UsbControlConfig::default()
            },
        );
        let mut controller = ControlClient::new(transport, &owner, NODE);
        let verified = controller.status(1).await.unwrap();
        let state = board_task.await.unwrap();

        assert_eq!(verified.counter, 1);
        assert_eq!(verified.known_good_generation, ConfigGeneration(3));
        assert_eq!(
            verified.status.authority(),
            ControlStatusAuthority::VerifiedController
        );
        assert_eq!(verified.status.node(), NODE);
        assert_eq!(verified.status.query_nonce(), verified.transaction.0);
        assert_eq!(state.owner_grants()[0].accepted_outer_counter(), 1);

        // The board's verifier, rebuilt from what it journaled, refuses that counter again.
        let mut rebuilt = restore_control_verifier(&state).unwrap();
        let replay = sign_request(
            &Request {
                transaction: verified.transaction,
                transaction_sequence: 0,
                expected_generation: ConfigGeneration(0),
                operation: Operation::Status,
                arguments: Vec::new(),
            },
            &owner,
            AddressHash::from_bytes(NODE.0),
            1,
        )
        .unwrap();
        assert_eq!(
            rebuilt.verify(&replay).err(),
            Some(retinue::command::Refusal::CounterReplayed)
        );
    }

    #[tokio::test]
    async fn diagnostic_authority_and_refusals_are_not_verified_status() {
        let owner = owner();
        let mut state = state(&owner);
        let (client, mut board) = tokio::io::duplex(2048);
        let board_task = tokio::spawn(async move {
            let frame = read_one_frame(&mut board).await;
            let response = board_answer(&mut state, &frame, true);
            write_response(&mut board, &response).await;
            let frame = read_one_frame(&mut board).await;
            let mut response = board_answer(&mut state, &frame, false);
            response.body = ResponseBody::Refused {
                reason: Refusal::UnsupportedOperation,
                result: Vec::new(),
            };
            write_response(&mut board, &response).await;
        });

        let transport = UsbControlTransport::from_io(
            client,
            UsbControlConfig {
                response_timeout: Duration::from_secs(2),
                ..UsbControlConfig::default()
            },
        );
        let mut controller = ControlClient::new(transport, &owner, NODE);
        assert!(matches!(
            controller.status(1).await,
            Err(ControlClientError::Authority)
        ));
        assert!(matches!(
            controller.status(2).await,
            Err(ControlClientError::Refused(Refusal::UnsupportedOperation))
        ));
        board_task.await.unwrap();
    }

    #[tokio::test]
    async fn silence_is_a_timeout_not_an_answer() {
        let owner = owner();
        let (client, mut board) = tokio::io::duplex(2048);
        let board_task = tokio::spawn(async move {
            let _ = read_one_frame(&mut board).await;
            board.write_all(b"unrelated text\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        });
        let transport = UsbControlTransport::from_io(
            client,
            UsbControlConfig {
                response_timeout: Duration::from_millis(100),
                ..UsbControlConfig::default()
            },
        );
        let mut controller = ControlClient::new(transport, &owner, NODE);
        assert!(matches!(
            controller.status(1).await,
            Err(ControlClientError::Carrier(UsbControlError::Timeout))
        ));
        board_task.await.unwrap();
    }
}
