use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;

use retinue::identity::PrivateIdentity;
use seneschal::control::{
    ClaimChallenge, ClaimResponse, FirstOwnerRequest, FirstOwnerResponse, FirstWriteStatus,
    INSPECT_RESPONSE_LEN, ManagementCarrier, NodeId, PairEvidence, ResumeResponse,
};
use seneschal::region::Region;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tulle::kiss;

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
    fn new(replies: impl IntoIterator<Item = Result<FirstOwnerResponse, &'static str>>) -> Self {
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
    let mut controller = FirstOwnerController::new(Script::new([Ok(FirstOwnerResponse::Claim(
        ClaimResponse::Rejected,
    ))]));
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
