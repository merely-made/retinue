//! The carrier-neutral signed control client and its receipts.

use std::future::Future;
use std::io;

use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use seneschal::control::{
    COMMIT_TOKEN_LEN, ChangeId, CommitArguments, ConfigGeneration, ControlStatusAuthority,
    ControlStatusError, ControlStatusV1, NodeId, Operation, ProvisionalApplyArguments,
    PublicConfigurationV1, Refusal, Request, Response, ResponseBody, RevertArguments,
    TransactionId,
};

use super::super::sign_request;

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
    Signing(#[source] super::super::Error),
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

/// The controller-side facts every mutable request carries beside its outer counter.
///
/// `sequence` is this controller's monotonic mutation sequence, which the board binds to
/// the semantic request so an evicted result can never make an old change executable
/// again; `expected_generation` is the known-good generation the controller last read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mutation {
    pub sequence: u64,
    pub expected_generation: ConfigGeneration,
}

/// The board's answer to a provisional apply: what a commit must name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProvisionalReceipt {
    pub transaction: TransactionId,
    pub counter: u64,
    pub change: ChangeId,
    pub candidate_generation: ConfigGeneration,
    pub deadline_ms: u64,
    pub commit_token: [u8; COMMIT_TOKEN_LEN],
}

impl std::fmt::Debug for ProvisionalReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionalReceipt")
            .field("transaction", &self.transaction)
            .field("counter", &self.counter)
            .field("change", &self.change)
            .field("candidate_generation", &self.candidate_generation)
            .field("deadline_ms", &self.deadline_ms)
            .field("commit_token", &"[redacted]")
            .finish()
    }
}

/// The board's answer to a commit or revert: the generation now known-good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedReceipt {
    pub transaction: TransactionId,
    pub counter: u64,
    pub known_good_generation: ConfigGeneration,
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

    /// Stages `public` as the provisional candidate with empty sealed credentials and
    /// applies it. The board rolls it back at `lifetime_ms` after applying, or on reboot,
    /// unless [`Self::commit`] names the returned generation and token first.
    pub async fn provisional_apply(
        &mut self,
        counter: u64,
        mutation: Mutation,
        change: ChangeId,
        public: PublicConfigurationV1,
        lifetime_ms: u64,
    ) -> Result<ProvisionalReceipt, ControlClientError<C::Error>> {
        let arguments = ProvisionalApplyArguments {
            change,
            public,
            lifetime_ms,
        }
        .encode();
        let request = self.mutation_request(Operation::ProvisionalApply, mutation, &arguments)?;
        let response = self.send(&request, counter).await?;
        let ResponseBody::Provisional {
            deadline_ms,
            commit_token,
            ..
        } = response.body
        else {
            return Err(ControlClientError::UnexpectedBody);
        };
        let candidate_generation = response
            .effective_generation
            .ok_or(ControlClientError::UnexpectedBody)?;
        Ok(ProvisionalReceipt {
            transaction: request.transaction,
            counter,
            change,
            candidate_generation,
            deadline_ms,
            commit_token,
        })
    }

    /// Confirms the exact armed candidate a [`ProvisionalReceipt`] describes.
    pub async fn commit(
        &mut self,
        counter: u64,
        mutation: Mutation,
        receipt: &ProvisionalReceipt,
    ) -> Result<AppliedReceipt, ControlClientError<C::Error>> {
        let arguments = CommitArguments {
            change: receipt.change,
            candidate_generation: receipt.candidate_generation,
            commit_token: receipt.commit_token,
        }
        .encode();
        let request = self.mutation_request(Operation::Commit, mutation, &arguments)?;
        self.applied(&request, counter).await
    }

    /// Abandons the armed candidate named by `change` and restores known-good now.
    pub async fn revert(
        &mut self,
        counter: u64,
        mutation: Mutation,
        change: ChangeId,
    ) -> Result<AppliedReceipt, ControlClientError<C::Error>> {
        let arguments = RevertArguments { change }.encode();
        let request = self.mutation_request(Operation::Revert, mutation, &arguments)?;
        self.applied(&request, counter).await
    }

    async fn applied(
        &mut self,
        request: &Request,
        counter: u64,
    ) -> Result<AppliedReceipt, ControlClientError<C::Error>> {
        let response = self.send(request, counter).await?;
        if !matches!(response.body, ResponseBody::Applied(_)) {
            return Err(ControlClientError::UnexpectedBody);
        }
        Ok(AppliedReceipt {
            transaction: request.transaction,
            counter,
            known_good_generation: response.known_good_generation,
        })
    }

    fn mutation_request(
        &self,
        operation: Operation,
        mutation: Mutation,
        arguments: &[u8],
    ) -> Result<Request, ControlClientError<C::Error>> {
        let mut transaction = [0_u8; 16];
        getrandom::fill(&mut transaction)
            .map_err(|error| ControlClientError::Entropy(io::Error::other(error.to_string())))?;
        Ok(Request {
            transaction: TransactionId(transaction),
            transaction_sequence: mutation.sequence,
            expected_generation: mutation.expected_generation,
            operation,
            arguments: heapless::Vec::try_from(arguments)
                .map_err(|_| ControlClientError::UnexpectedBody)?,
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
