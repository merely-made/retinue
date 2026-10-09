//! The carrier-neutral first-owner controller and its outcomes.

use std::future::Future;

use retinue::identity::PrivateIdentity;
use seneschal::control::{
    AbandonResponse, CLAIM_PROOF_LEN, ClaimRequest, ClaimResponse, FirstOwnerRequest,
    FirstOwnerResponse, FirstWriteActions, FirstWriteEligibility, FirstWriteStatus, NodeId,
    OwnerClaim, OwnerClaimError, ResumeResponse, claim_proof_transcript,
};

use super::ClaimPlan;

/// Carrier-neutral request/reply boundary for first-owner setup.
///
/// Implementations receive only portable public claim bytes, encoded through
/// [`FirstOwnerRequest`].  In particular, the signing identity stays with the
/// caller of [`FirstOwnerController::claim`].
pub trait FirstOwnerExchange {
    type Error;

    fn exchange(
        &mut self,
        request: FirstOwnerRequest,
    ) -> impl Future<Output = Result<FirstOwnerResponse, Self::Error>>;
}

/// A fresh board inspection. The challenge nonce is intentionally private to this module.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Inspection {
    status: FirstWriteStatus,
    node: NodeId,
    nonce: [u8; 32],
}

impl core::fmt::Debug for Inspection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Inspection")
            .field("status", &self.status)
            .field("node", &self.node)
            .field("nonce", &"[redacted]")
            .finish()
    }
}

impl Inspection {
    pub const fn status(self) -> FirstWriteStatus {
        self.status
    }

    pub const fn node(self) -> NodeId {
        self.node
    }

    pub const fn eligibility(self) -> FirstWriteEligibility {
        self.status.eligibility()
    }

    pub const fn actions(self) -> FirstWriteActions {
        self.status.actions()
    }
}

/// Distinct terminal outcomes. Cleanup-pending is visible because an unplug after it is not
/// evidence that the board erased the scratch copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    Committed,
    CommittedCleanupPending,
}

/// Outcome of an explicit Resume action.
///
/// Claim refuses pre-existing pending work so the operator must choose Resume or Abandon. Once
/// Claim has staged its own fresh request, it sends exactly one Resume to reach a terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeOutcome {
    Committed,
    CommittedCleanupPending,
}

/// Why a controller workflow ended without a claimed node.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FirstOwnerError<E> {
    #[error("first-owner carrier failure")]
    Carrier(E),
    #[error("first-owner response did not match {expected}")]
    UnexpectedResponse { expected: &'static str },
    #[error("first-owner claim was rejected")]
    ClaimRejected,
    #[error("first-owner resume was rejected")]
    ResumeRejected,
    #[error("first-owner abandon was rejected")]
    AbandonRejected,
    #[error("node already has control state")]
    ControlPresent,
    #[error("node first-write state is faulty")]
    Fault,
    #[error("node has pending first-owner work; explicitly resume or abandon it")]
    NeedsRecovery(Inspection),
    #[error("claim is not eligible in the inspected state")]
    ClaimIneligible,
    #[error("resume is not eligible in the inspected state")]
    ResumeIneligible,
    #[error("abandon is not eligible in the inspected state")]
    AbandonIneligible,
    #[error("owner claim is invalid: {0:?}")]
    InvalidClaim(OwnerClaimError),
    #[error("claim reply was lost; inspect and explicitly resume or abandon if it staged")]
    ClaimNeedsRecovery(E),
    #[error("resume reply was lost; inspect before trying another recovery action")]
    ResumeNeedsRecovery(E),
    #[error("abandon reply was lost; inspect before trying another recovery action")]
    AbandonNeedsRecovery(E),
    #[error("claim was staged, then the terminal resume outcome became uncertain")]
    StagedNeedsRecovery(E),
    #[error("claim was staged, then Resume did not confirm a terminal commit")]
    StagedRecoveryRequired,
}

/// The carrier-neutral controller. It owns the single-use challenge lifecycle.
pub struct FirstOwnerController<C> {
    carrier: C,
}

impl<C> FirstOwnerController<C> {
    pub fn new(carrier: C) -> Self {
        Self { carrier }
    }

    pub fn into_carrier(self) -> C {
        self.carrier
    }
}

impl<C> FirstOwnerController<C>
where
    C: FirstOwnerExchange,
{
    /// Inspect without changing the board. A caller can display its status and decide the
    /// explicit recovery action that follows.
    pub async fn inspect(&mut self) -> Result<Inspection, FirstOwnerError<C::Error>> {
        match self
            .carrier
            .exchange(FirstOwnerRequest::Inspect)
            .await
            .map_err(FirstOwnerError::Carrier)?
        {
            FirstOwnerResponse::Inspect {
                status,
                node,
                nonce,
            } => Ok(Inspection {
                status,
                node,
                nonce,
            }),
            _ => Err(FirstOwnerError::UnexpectedResponse {
                expected: "Inspect",
            }),
        }
    }

    /// Freshly inspect, claim exactly that nonce once, then resume a staged claim once.
    pub async fn claim(
        &mut self,
        identity: &PrivateIdentity,
        plan: ClaimPlan,
    ) -> Result<ClaimOutcome, FirstOwnerError<C::Error>> {
        let inspected = self.inspect().await?;
        match inspected.eligibility() {
            FirstWriteEligibility::Uncommissioned => {}
            FirstWriteEligibility::Resume => return Err(FirstOwnerError::NeedsRecovery(inspected)),
            FirstWriteEligibility::ControlPresent => return Err(FirstOwnerError::ControlPresent),
            FirstWriteEligibility::Fault => return Err(FirstOwnerError::Fault),
        }
        if !inspected.actions().permits_claim() {
            return Err(FirstOwnerError::ClaimIneligible);
        }

        let public_identity = identity.public().to_public_bytes();
        let claim = OwnerClaim::new(
            &public_identity,
            plan.public_configuration(),
            plan.recovery_policy(),
        )
        .map_err(FirstOwnerError::InvalidClaim)?;
        let mut transcript = [0_u8; CLAIM_PROOF_LEN];
        claim_proof_transcript(inspected.node, inspected.nonce, &claim, &mut transcript);
        let signature = identity.sign(&transcript);
        let request = ClaimRequest::new(inspected.node, inspected.nonce, claim, signature);
        let response = self
            .carrier
            .exchange(FirstOwnerRequest::Claim(request))
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => return Err(FirstOwnerError::ClaimNeedsRecovery(error)),
        };
        match response {
            FirstOwnerResponse::Claim(ClaimResponse::Rejected) => {
                return Err(FirstOwnerError::ClaimRejected);
            }
            FirstOwnerResponse::Claim(ClaimResponse::Staged) => {}
            _ => {
                return Err(FirstOwnerError::UnexpectedResponse { expected: "Claim" });
            }
        }
        // A timeout or detach here is intentionally uncertain. A retry could race a successful
        // board commit after its reply and must be resolved by a new explicit inspection.
        let response = self.carrier.exchange(FirstOwnerRequest::Resume).await;
        let response = match response {
            Ok(response) => response,
            Err(error) => return Err(FirstOwnerError::StagedNeedsRecovery(error)),
        };
        match response {
            FirstOwnerResponse::Resume(ResumeResponse::Committed) => Ok(ClaimOutcome::Committed),
            FirstOwnerResponse::Resume(ResumeResponse::CommittedCleanupPending) => {
                Ok(ClaimOutcome::CommittedCleanupPending)
            }
            FirstOwnerResponse::Resume(ResumeResponse::Rejected) => {
                Err(FirstOwnerError::StagedRecoveryRequired)
            }
            _ => Err(FirstOwnerError::StagedRecoveryRequired),
        }
    }

    /// Resume only after an explicit inspection says it is eligible.
    pub async fn resume(&mut self) -> Result<ResumeOutcome, FirstOwnerError<C::Error>> {
        let inspected = self.inspect().await?;
        if !inspected.actions().permits_resume() {
            return Err(match inspected.eligibility() {
                FirstWriteEligibility::ControlPresent => FirstOwnerError::ControlPresent,
                FirstWriteEligibility::Fault => FirstOwnerError::Fault,
                FirstWriteEligibility::Uncommissioned | FirstWriteEligibility::Resume => {
                    FirstOwnerError::ResumeIneligible
                }
            });
        }
        match self
            .carrier
            .exchange(FirstOwnerRequest::Resume)
            .await
            .map_err(FirstOwnerError::ResumeNeedsRecovery)?
        {
            FirstOwnerResponse::Resume(ResumeResponse::Committed) => Ok(ResumeOutcome::Committed),
            FirstOwnerResponse::Resume(ResumeResponse::CommittedCleanupPending) => {
                Ok(ResumeOutcome::CommittedCleanupPending)
            }
            FirstOwnerResponse::Resume(ResumeResponse::Rejected) => {
                Err(FirstOwnerError::ResumeRejected)
            }
            _ => Err(FirstOwnerError::UnexpectedResponse { expected: "Resume" }),
        }
    }

    /// Abandon only after an explicit inspection says it is eligible.
    pub async fn abandon(&mut self) -> Result<(), FirstOwnerError<C::Error>> {
        let inspected = self.inspect().await?;
        if !inspected.actions().permits_abandon() {
            return Err(match inspected.eligibility() {
                FirstWriteEligibility::ControlPresent => FirstOwnerError::ControlPresent,
                FirstWriteEligibility::Fault => FirstOwnerError::Fault,
                FirstWriteEligibility::Uncommissioned | FirstWriteEligibility::Resume => {
                    FirstOwnerError::AbandonIneligible
                }
            });
        }
        match self
            .carrier
            .exchange(FirstOwnerRequest::Abandon)
            .await
            .map_err(FirstOwnerError::AbandonNeedsRecovery)?
        {
            FirstOwnerResponse::Abandon(AbandonResponse::Abandoned) => Ok(()),
            FirstOwnerResponse::Abandon(AbandonResponse::Rejected) => {
                Err(FirstOwnerError::AbandonRejected)
            }
            _ => Err(FirstOwnerError::UnexpectedResponse {
                expected: "Abandon",
            }),
        }
    }
}
