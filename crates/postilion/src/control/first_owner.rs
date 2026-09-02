//! Controller and literal USB carrier for a wall node's first owner.
//!
//! This is deliberately below a Signalman face and above serial/KISS.  A
//! carrier exchanges already-built portable requests; it never receives a
//! private identity or decides what a prospective owner should configure.

use std::future::Future;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use radio_hand::control::{
    AbandonResponse, CLAIM_PROOF_LEN, CLAIM_REQUEST_LEN, ClaimRequest, ClaimResponse,
    FirstOwnerRequest, FirstOwnerResponse, FirstOwnerWireError, FirstWriteActions,
    FirstWriteEligibility, FirstWriteStatus, INSPECT_RESPONSE_LEN, ManagementCarrier,
    ManagementCarrierSet, NodeId, OwnerClaim, OwnerClaimError, PublicConfigurationV1,
    RecoveryClause, RecoveryPolicy, RecoveryPolicyError, ResumeResponse, ReticulumTransportPolicy,
    claim_proof_transcript,
};
use radio_hand::region::Region;
use retinue::identity::PrivateIdentity;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tulle::PhyProfile;
use tulle::kiss;

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

/// Public configuration and recovery policy selected by the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaimPlan {
    public_configuration: PublicConfigurationV1,
    recovery_policy: RecoveryPolicy,
}

impl ClaimPlan {
    pub const fn new(
        public_configuration: PublicConfigurationV1,
        recovery_policy: RecoveryPolicy,
    ) -> Self {
        Self {
            public_configuration,
            recovery_policy,
        }
    }

    pub const fn public_configuration(self) -> PublicConfigurationV1 {
        self.public_configuration
    }

    pub const fn recovery_policy(self) -> RecoveryPolicy {
        self.recovery_policy
    }
}

/// Constructs the only truthful first-owner plan the current V4 USB carrier can make.
///
/// It enables USB management only, establishes one physical-presence USB recovery clause,
/// leaves authenticated remote recovery disabled, provisions no credentials, and makes this
/// first configuration a non-relay Reticulum transport.
pub fn v4_usb_claim_plan(region: Region, phy: PhyProfile) -> Result<ClaimPlan, V4UsbPlanError> {
    let carriers = ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8)
        .expect("the USB management bit is defined and non-empty");
    let transport = ReticulumTransportPolicy::new(false, false, 0)
        .expect("a non-relay, zero-hop policy is canonical");
    let public = PublicConfigurationV1::new(region, phy, transport, carriers)
        .map_err(V4UsbPlanError::Configuration)?;
    let physical = RecoveryClause::new(carriers, 1).expect("one USB survivor is canonical");
    let policy = RecoveryPolicy::new(physical, RecoveryClause::disabled())
        .map_err(V4UsbPlanError::Recovery)?;
    Ok(ClaimPlan::new(public, policy))
}

/// A V4 public claim-plan input was invalid before it touched a board.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum V4UsbPlanError {
    #[error("invalid V4 public configuration: {0:?}")]
    Configuration(radio_hand::control::PublicConfigurationError),
    #[error("invalid V4 USB recovery policy: {0:?}")]
    Recovery(RecoveryPolicyError),
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
                for frame in frames {
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

    fn exchange(
        &mut self,
        request: FirstOwnerRequest,
    ) -> impl Future<Output = Result<FirstOwnerResponse, Self::Error>> {
        async move {
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use radio_hand::control::{ClaimChallenge, PairEvidence};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const NODE: NodeId = NodeId([0x22; 16]);

    fn identity() -> PrivateIdentity {
        PrivateIdentity::from_secret_bytes(&[0x31; 64])
    }

    fn inspection(control: PairEvidence, pending: PairEvidence) -> FirstOwnerResponse {
        FirstOwnerResponse::Inspect {
            status: FirstWriteStatus { control, pending },
            node: NODE,
            nonce: [0x51; 32],
        }
    }

    fn plan() -> ClaimPlan {
        let mut phy = crate::profile(250_000);
        phy.frequency_hz = 906_875_000;
        v4_usb_claim_plan(Region::Us915, phy).unwrap()
    }

    #[test]
    fn v4_usb_claim_plan_is_usb_only_non_relay_with_physical_recovery() {
        let plan = plan();
        let public = plan.public_configuration();
        let carriers = public.enabled_management_carriers();
        assert!(carriers.contains(ManagementCarrier::Usb));
        assert!(!carriers.contains(ManagementCarrier::Ble));
        assert!(!carriers.contains(ManagementCarrier::Ip));
        assert!(!carriers.contains(ManagementCarrier::Reticulum));

        let transport = public.reticulum_transport();
        assert!(!transport.relay_announces);
        assert!(!transport.relay_packets);
        assert_eq!(transport.max_hops, 0);

        let physical = plan.recovery_policy().physical_presence();
        assert_eq!(
            physical.acceptable_mask(),
            1 << ManagementCarrier::Usb as u8
        );
        assert_eq!(physical.minimum_survivors(), 1);

        let remote = plan.recovery_policy().authenticated_remote();
        assert_eq!(remote.acceptable_mask(), 0);
        assert_eq!(remote.minimum_survivors(), 0);
    }

    struct Script {
        replies: VecDeque<Result<FirstOwnerResponse, &'static str>>,
        requests: Vec<FirstOwnerRequest>,
    }

    impl Script {
        fn new(
            replies: impl IntoIterator<Item = Result<FirstOwnerResponse, &'static str>>,
        ) -> Self {
            Self {
                replies: replies.into_iter().collect(),
                requests: Vec::new(),
            }
        }
    }

    impl FirstOwnerExchange for Script {
        type Error = &'static str;

        fn exchange(
            &mut self,
            request: FirstOwnerRequest,
        ) -> impl Future<Output = Result<FirstOwnerResponse, Self::Error>> {
            self.requests.push(request);
            std::future::ready(self.replies.pop_front().expect("script reply"))
        }
    }

    #[tokio::test]
    async fn claim_freshly_inspects_signs_the_exact_contract_and_resumes_once() {
        let owner = identity();
        let mut controller = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Blank)),
            Ok(FirstOwnerResponse::Claim(ClaimResponse::Staged)),
            Ok(FirstOwnerResponse::Resume(ResumeResponse::Committed)),
        ]));
        assert_eq!(
            controller.claim(&owner, plan()).await,
            Ok(ClaimOutcome::Committed)
        );
        let script = controller.into_carrier();
        assert_eq!(script.requests.len(), 3);
        let FirstOwnerRequest::Claim(request) = &script.requests[1] else {
            panic!("the fresh inspect must be followed by claim");
        };
        assert_eq!(
            ClaimChallenge::from_fresh_entropy([0x51; 32]).verify(request, NODE),
            Ok(request.claim().clone())
        );
        assert!(matches!(script.requests[2], FirstOwnerRequest::Resume));
    }

    #[tokio::test]
    async fn claim_refuses_pending_without_sending_claim_or_resume() {
        let owner = identity();
        let mut controller = FirstOwnerController::new(Script::new([Ok(inspection(
            PairEvidence::Blank,
            PairEvidence::Valid,
        ))]));
        assert!(matches!(
            controller.claim(&owner, plan()).await,
            Err(FirstOwnerError::NeedsRecovery(_))
        ));
        assert_eq!(controller.into_carrier().requests.len(), 1);
    }

    #[tokio::test]
    async fn cleanup_pending_is_not_collapsed_into_committed() {
        let owner = identity();
        let mut controller = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Blank)),
            Ok(FirstOwnerResponse::Claim(ClaimResponse::Staged)),
            Ok(FirstOwnerResponse::Resume(
                ResumeResponse::CommittedCleanupPending,
            )),
        ]));
        assert_eq!(
            controller.claim(&owner, plan()).await,
            Ok(ClaimOutcome::CommittedCleanupPending)
        );
    }

    #[tokio::test]
    async fn claim_rejection_and_staged_carrier_failure_do_not_retry() {
        let owner = identity();
        let mut rejected = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Blank)),
            Ok(FirstOwnerResponse::Claim(ClaimResponse::Rejected)),
        ]));
        assert_eq!(
            rejected.claim(&owner, plan()).await,
            Err(FirstOwnerError::ClaimRejected)
        );
        assert_eq!(rejected.into_carrier().requests.len(), 2);

        let mut uncertain = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Blank)),
            Ok(FirstOwnerResponse::Claim(ClaimResponse::Staged)),
            Err("detach"),
        ]));
        assert_eq!(
            uncertain.claim(&owner, plan()).await,
            Err(FirstOwnerError::StagedNeedsRecovery("detach"))
        );
        assert_eq!(uncertain.into_carrier().requests.len(), 3);

        let immediate_loss = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Blank)),
            Err("lost-claim-reply"),
        ]));
        let mut immediate_loss = immediate_loss;
        assert_eq!(
            immediate_loss.claim(&owner, plan()).await,
            Err(FirstOwnerError::ClaimNeedsRecovery("lost-claim-reply"))
        );
        assert_eq!(immediate_loss.into_carrier().requests.len(), 2);

        let mut staged_rejected = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Blank)),
            Ok(FirstOwnerResponse::Claim(ClaimResponse::Staged)),
            Ok(FirstOwnerResponse::Resume(ResumeResponse::Rejected)),
        ]));
        assert_eq!(
            staged_rejected.claim(&owner, plan()).await,
            Err(FirstOwnerError::StagedRecoveryRequired)
        );
        assert_eq!(staged_rejected.into_carrier().requests.len(), 3);
    }

    #[tokio::test]
    async fn explicit_resume_and_abandon_require_the_inspected_action() {
        let mut resume = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Valid)),
            Ok(FirstOwnerResponse::Resume(ResumeResponse::Committed)),
        ]));
        assert_eq!(resume.resume().await, Ok(ResumeOutcome::Committed));
        assert_eq!(resume.into_carrier().requests.len(), 2);

        let mut abandon = FirstOwnerController::new(Script::new([Ok(inspection(
            PairEvidence::Blank,
            PairEvidence::Blank,
        ))]));
        assert_eq!(
            abandon.abandon().await,
            Err(FirstOwnerError::AbandonIneligible)
        );
        assert_eq!(abandon.into_carrier().requests.len(), 1);

        let mut lost_resume = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Valid)),
            Err("lost-resume-reply"),
        ]));
        assert_eq!(
            lost_resume.resume().await,
            Err(FirstOwnerError::ResumeNeedsRecovery("lost-resume-reply"))
        );
        assert_eq!(lost_resume.into_carrier().requests.len(), 2);

        let mut lost_abandon = FirstOwnerController::new(Script::new([
            Ok(inspection(PairEvidence::Blank, PairEvidence::Corrupt)),
            Err("lost-abandon-reply"),
        ]));
        assert_eq!(
            lost_abandon.abandon().await,
            Err(FirstOwnerError::AbandonNeedsRecovery("lost-abandon-reply"))
        );
        assert_eq!(lost_abandon.into_carrier().requests.len(), 2);
    }

    #[tokio::test]
    async fn wrong_response_kinds_are_typed() {
        let mut controller = FirstOwnerController::new(Script::new([Ok(
            FirstOwnerResponse::Claim(ClaimResponse::Rejected),
        )]));
        assert!(matches!(
            controller.inspect().await,
            Err(FirstOwnerError::UnexpectedResponse {
                expected: "Inspect"
            })
        ));
    }

    fn response_bytes(response: FirstOwnerResponse) -> Vec<u8> {
        let mut bytes = [0; INSPECT_RESPONSE_LEN];
        let length = response
            .encode(
                &mut bytes[..match response {
                    FirstOwnerResponse::Inspect { .. } => INSPECT_RESPONSE_LEN,
                    _ => 3,
                }],
            )
            .unwrap();
        kiss::encode(&bytes[..length])
    }

    #[tokio::test]
    async fn usb_transport_handles_fragmentation_noise_and_resync() {
        let (host, mut board) = tokio::io::duplex(4096);
        let board_task = tokio::spawn(async move {
            let mut request = [0; 512];
            let _ = board.read(&mut request).await.unwrap();
            board
                .write_all(&[kiss::FEND, 1, kiss::FESC, 0x01, kiss::FEND])
                .await
                .unwrap();
            let mut oversize = vec![kiss::FEND];
            oversize.extend(std::iter::repeat_n(0x44, INSPECT_RESPONSE_LEN + 1));
            oversize.push(kiss::FEND);
            board.write_all(&oversize).await.unwrap();
            let bytes = response_bytes(inspection(PairEvidence::Blank, PairEvidence::Blank));
            for fragment in bytes.chunks(3) {
                board.write_all(fragment).await.unwrap();
            }
        });
        let mut transport = UsbFirstOwnerTransport::from_io(host, UsbFirstOwnerConfig::default());
        assert!(matches!(
            transport.exchange(FirstOwnerRequest::Inspect).await,
            Ok(FirstOwnerResponse::Inspect { .. })
        ));
        board_task.await.unwrap();
    }

    #[tokio::test]
    async fn usb_transport_rejects_a_well_framed_malformed_response() {
        let (host, mut board) = tokio::io::duplex(128);
        let task = tokio::spawn(async move {
            let mut request = [0; 128];
            let _ = board.read(&mut request).await.unwrap();
            board.write_all(&kiss::encode(&[1, 0x81, 9])).await.unwrap();
        });
        let mut transport = UsbFirstOwnerTransport::from_io(host, UsbFirstOwnerConfig::default());
        assert!(matches!(
            transport.exchange(FirstOwnerRequest::Inspect).await,
            Err(UsbFirstOwnerError::Malformed(_))
        ));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn usb_transport_rejects_mismatch_timeout_and_eof() {
        let (host, mut board) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            let mut request = [0; 512];
            let _ = board.read(&mut request).await.unwrap();
            board
                .write_all(&response_bytes(FirstOwnerResponse::Claim(
                    ClaimResponse::Rejected,
                )))
                .await
                .unwrap();
        });
        let mut transport = UsbFirstOwnerTransport::from_io(host, UsbFirstOwnerConfig::default());
        assert!(matches!(
            transport.exchange(FirstOwnerRequest::Inspect).await,
            Err(UsbFirstOwnerError::MismatchedResponse)
        ));
        task.await.unwrap();

        let (host, board) = tokio::io::duplex(128);
        drop(board);
        let mut eof = UsbFirstOwnerTransport::from_io(host, UsbFirstOwnerConfig::default());
        assert!(matches!(
            eof.exchange(FirstOwnerRequest::Inspect).await,
            Err(UsbFirstOwnerError::Eof | UsbFirstOwnerError::Io(_))
        ));

        let (host, mut board) = tokio::io::duplex(128);
        let mut timeout = UsbFirstOwnerTransport::from_io(
            host,
            UsbFirstOwnerConfig {
                response_timeout: Duration::from_millis(1),
                ..UsbFirstOwnerConfig::default()
            },
        );
        assert!(matches!(
            timeout.exchange(FirstOwnerRequest::Inspect).await,
            Err(UsbFirstOwnerError::Timeout)
        ));
        assert!(matches!(
            timeout.exchange(FirstOwnerRequest::Inspect).await,
            Err(UsbFirstOwnerError::ReconnectRequired)
        ));
        let mut first_request = [0; 128];
        assert!(
            tokio::time::timeout(Duration::from_millis(100), board.read(&mut first_request))
                .await
                .expect("the first request was written")
                .expect("the board side remains live")
                > 0
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), board.read(&mut first_request))
                .await
                .is_err()
        );
    }

    #[test]
    fn v4_usb_defaults_keep_control_lines_deasserted() {
        let config = UsbFirstOwnerConfig::default();
        assert_eq!(config.baud_rate, 115_200);
        assert!(!config.dtr());
        assert!(!config.rts());
        assert_eq!(config.session_timeout, Duration::from_secs(45));
    }
}
