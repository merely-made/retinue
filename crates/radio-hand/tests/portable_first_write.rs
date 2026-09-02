use ed25519_dalek::{Signer, SigningKey};
use heapless::Vec;
use radio_hand::control::{
    AbandonOutcome, BoardRecoveryFacts, CLAIM_PROOF_LEN, CLAIM_REQUEST_LEN, ClaimChallenge,
    ClaimProofError, ClaimRequest, DurableState, FIRST_OWNER_VERSION, FirstOwnerRequest,
    FirstOwnerResponse, FirstWriteActions, FirstWriteScratch, FirstWriteStatus,
    FirstWriteStorageError, FirstWriteStore, INSPECT_RESPONSE_LEN, ManagementCarrier,
    ManagementCarrierSet, NodeId, OwnerClaim, PairEvidence, PublicConfigurationV1, RecoveryClause,
    RecoveryPathFacts, RecoveryPolicy, ResumeOutcome, StageOutcome, abandon_first_write,
    claim_proof_transcript, first_write_status, resume_first_write, stage_first_write,
};
use radio_hand::region::Region;
use radio_hand::store::Slot;

const PAGE: usize = 4096;
const NODE: NodeId = NodeId([0x10; 16]);

fn identity(seed: u8) -> [u8; 64] {
    let mut identity = [seed; 64];
    identity[32..].copy_from_slice(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .as_bytes(),
    );
    identity
}

fn configuration() -> PublicConfigurationV1 {
    PublicConfigurationV1::new(
        Region::Us915,
        selvage::PhyProfile::meshtastic_long_fast(906_875_000),
        radio_hand::control::ReticulumTransportPolicy::new(false, false, 0).unwrap(),
        ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8).unwrap(),
    )
    .unwrap()
}

fn policy() -> RecoveryPolicy {
    RecoveryPolicy::new(
        RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
        RecoveryClause::disabled(),
    )
    .unwrap()
}

fn facts() -> BoardRecoveryFacts {
    BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap()
}

fn claim() -> OwnerClaim {
    OwnerClaim::new(&identity(0x31), configuration(), policy()).unwrap()
}

fn state() -> DurableState {
    DurableState::from_owner_claim(NODE, claim(), &facts()).unwrap()
}

fn signed_request(node: NodeId, nonce: [u8; 32], claim: OwnerClaim) -> ClaimRequest {
    let mut transcript = [0; CLAIM_PROOF_LEN];
    claim_proof_transcript(node, nonce, &claim, &mut transcript);
    let signature = SigningKey::from_bytes(&[0x31; 32])
        .sign(&transcript)
        .to_bytes();
    ClaimRequest::new(node, nonce, claim, signature)
}

#[test]
fn exact_wire_round_trips_and_rejects_truncation_trailing_versions_and_opcodes() {
    let request = FirstOwnerRequest::Claim(signed_request(NODE, [0x43; 32], claim()));
    let mut bytes = [0; CLAIM_REQUEST_LEN];
    assert_eq!(request.encode(&mut bytes), Ok(CLAIM_REQUEST_LEN));
    assert_eq!(FirstOwnerRequest::decode(&bytes), Ok(request.clone()));
    assert!(FirstOwnerRequest::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = [0; CLAIM_REQUEST_LEN + 1];
    trailing[..CLAIM_REQUEST_LEN].copy_from_slice(&bytes);
    assert!(FirstOwnerRequest::decode(&trailing).is_err());
    bytes[0] = FIRST_OWNER_VERSION + 1;
    assert!(FirstOwnerRequest::decode(&bytes).is_err());
    bytes[0] = FIRST_OWNER_VERSION;
    bytes[1] = 0x7f;
    assert!(FirstOwnerRequest::decode(&bytes).is_err());

    let response = FirstOwnerResponse::Inspect {
        status: FirstWriteStatus {
            control: PairEvidence::Blank,
            pending: PairEvidence::Blank,
        },
        node: NODE,
        nonce: [0x52; 32],
    };
    let mut response_bytes = [0; INSPECT_RESPONSE_LEN];
    assert_eq!(
        response.encode(&mut response_bytes),
        Ok(INSPECT_RESPONSE_LEN)
    );
    assert_eq!(FirstOwnerResponse::decode(&response_bytes), Ok(response));
    response_bytes[4] = 3;
    assert!(FirstOwnerResponse::decode(&response_bytes).is_err());
    let response = FirstOwnerResponse::Inspect {
        status: FirstWriteStatus {
            control: PairEvidence::Blank,
            pending: PairEvidence::Blank,
        },
        node: NODE,
        nonce: [0x52; 32],
    };
    let mut response_bytes = [0; INSPECT_RESPONSE_LEN];
    response.encode(&mut response_bytes).unwrap();
    assert!(FirstOwnerResponse::decode(&response_bytes[..INSPECT_RESPONSE_LEN - 1]).is_err());
    let mut trailing_response = [0; INSPECT_RESPONSE_LEN + 1];
    trailing_response[..INSPECT_RESPONSE_LEN].copy_from_slice(&response_bytes);
    assert!(FirstOwnerResponse::decode(&trailing_response).is_err());
    assert!(!format!("{response:?}").contains("82"));
}

#[test]
fn every_simple_request_and_response_variant_is_exact_and_rejects_bad_dispositions() {
    for request in [
        FirstOwnerRequest::Inspect,
        FirstOwnerRequest::Resume,
        FirstOwnerRequest::Abandon,
    ] {
        let mut bytes = [0; 2];
        assert_eq!(request.encode(&mut bytes), Ok(2));
        assert_eq!(FirstOwnerRequest::decode(&bytes), Ok(request));
        assert!(FirstOwnerRequest::decode(&[bytes[0], bytes[1], 0]).is_err());
    }
    for response in [
        FirstOwnerResponse::Claim(radio_hand::control::ClaimResponse::Rejected),
        FirstOwnerResponse::Claim(radio_hand::control::ClaimResponse::Staged),
        FirstOwnerResponse::Resume(radio_hand::control::ResumeResponse::Rejected),
        FirstOwnerResponse::Resume(radio_hand::control::ResumeResponse::Committed),
        FirstOwnerResponse::Resume(radio_hand::control::ResumeResponse::CommittedCleanupPending),
        FirstOwnerResponse::Abandon(radio_hand::control::AbandonResponse::Rejected),
        FirstOwnerResponse::Abandon(radio_hand::control::AbandonResponse::Abandoned),
    ] {
        let mut bytes = [0; 3];
        assert_eq!(response.encode(&mut bytes), Ok(3));
        assert_eq!(FirstOwnerResponse::decode(&bytes), Ok(response));
        assert!(FirstOwnerResponse::decode(&bytes[..2]).is_err());
        assert!(FirstOwnerResponse::decode(&[bytes[0], bytes[1], bytes[2], 0]).is_err());
    }
    assert!(FirstOwnerResponse::decode(&[FIRST_OWNER_VERSION, 0x82, 0x7f]).is_err());
    assert!(FirstOwnerResponse::decode(&[FIRST_OWNER_VERSION, 0x83, 0x7f]).is_err());
    assert!(FirstOwnerResponse::decode(&[FIRST_OWNER_VERSION, 0x84, 0x7f]).is_err());
}

#[test]
fn inspect_action_bits_are_exact_for_every_recovery_shape() {
    let blank = FirstWriteStatus {
        control: PairEvidence::Blank,
        pending: PairEvidence::Blank,
    }
    .actions();
    assert!(blank.permits_claim() && blank.permits_ordinary_service());
    assert!(!blank.permits_resume() && !blank.permits_abandon());
    let pending = FirstWriteStatus {
        control: PairEvidence::Blank,
        pending: PairEvidence::Valid,
    }
    .actions();
    assert!(pending.permits_resume() && pending.permits_abandon());
    let corrupt_pending = FirstWriteStatus {
        control: PairEvidence::Blank,
        pending: PairEvidence::Corrupt,
    }
    .actions();
    assert_eq!(corrupt_pending.bits(), FirstWriteActions::ABANDON);
    let repair = FirstWriteStatus {
        control: PairEvidence::Corrupt,
        pending: PairEvidence::Valid,
    }
    .actions();
    assert_eq!(repair.bits(), FirstWriteActions::RESUME);
}

#[test]
fn claim_proof_binds_every_authority_bearing_byte_and_is_one_shot() {
    let nonce = [0x81; 32];
    let request = signed_request(NODE, nonce, claim());
    assert_eq!(
        ClaimChallenge::from_fresh_entropy(nonce).verify(&request, NODE),
        Ok(claim())
    );
    assert_eq!(
        ClaimChallenge::from_fresh_entropy(nonce).verify(&request, NodeId([0x11; 16])),
        Err(ClaimProofError::WrongNode)
    );
    assert_eq!(
        ClaimChallenge::from_fresh_entropy([0x82; 32]).verify(&request, NODE),
        Err(ClaimProofError::WrongNonce)
    );

    let changed_config = OwnerClaim::new(
        &identity(0x31),
        PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(907_875_000),
            radio_hand::control::ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap(),
        policy(),
    )
    .unwrap();
    let changed_recovery = OwnerClaim::new(
        &identity(0x31),
        configuration(),
        RecoveryPolicy::new(
            RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
            RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut changed_x25519 = identity(0x31);
    changed_x25519[0] ^= 1;
    let mut changed_ed25519 = identity(0x31);
    changed_ed25519[32..].copy_from_slice(
        SigningKey::from_bytes(&[0x32; 32])
            .verifying_key()
            .as_bytes(),
    );
    for changed in [
        changed_config,
        changed_recovery,
        OwnerClaim::new(&changed_x25519, configuration(), policy()).unwrap(),
        OwnerClaim::new(&changed_ed25519, configuration(), policy()).unwrap(),
    ] {
        let tampered = ClaimRequest::new(NODE, nonce, changed, *request.signature());
        assert_eq!(
            ClaimChallenge::from_fresh_entropy(nonce).verify(&tampered, NODE),
            Err(ClaimProofError::InvalidSignature)
        );
    }
    let mut malformed_identity = identity(0x31);
    malformed_identity[32..].fill(2);
    assert!(OwnerClaim::new(&malformed_identity, configuration(), policy()).is_err());
    let bad_signature = ClaimRequest::new(NODE, nonce, claim(), [0; 64]);
    assert_eq!(
        ClaimChallenge::from_fresh_entropy(nonce).verify(&bad_signature, NODE),
        Err(ClaimProofError::InvalidSignature)
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    ReadControl,
    ReadPending,
    EraseControl,
    ProgramControl,
    VerifyControl,
    ErasePending,
    ProgramPending,
    VerifyPending,
    TornProgramControl,
    TornProgramPending,
    ErasePendingB,
    VerifyPendingB,
}

#[derive(Clone)]
struct Store {
    control: [[u8; PAGE]; 2],
    pending: [[u8; PAGE]; 2],
    fault: Fault,
    control_reads: u8,
    pending_reads: u8,
}

impl Store {
    fn blank() -> Self {
        Self {
            control: [[0xff; PAGE]; 2],
            pending: [[0xff; PAGE]; 2],
            fault: Fault::None,
            control_reads: 0,
            pending_reads: 0,
        }
    }
    fn slot(slot: Slot) -> usize {
        match slot {
            Slot::A => 0,
            Slot::B => 1,
        }
    }
    fn fail(&self, value: Fault) -> bool {
        self.fault == value
    }
    fn inject(&mut self, fault: Fault) {
        self.fault = fault;
        self.control_reads = 0;
        self.pending_reads = 0;
    }
}

impl FirstWriteStore for Store {
    type Error = Fault;
    fn read_control(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ReadControl)
            || self.fail(Fault::VerifyControl) && self.control_reads >= 2
        {
            return Err(if self.fault == Fault::ReadControl {
                Fault::ReadControl
            } else {
                Fault::VerifyControl
            });
        }
        out.copy_from_slice(&self.control[Self::slot(slot)]);
        self.control_reads += 1;
        Ok(())
    }
    fn erase_control(&mut self, slot: Slot) -> Result<(), Self::Error> {
        if self.fail(Fault::EraseControl) {
            return Err(Fault::EraseControl);
        }
        self.control[Self::slot(slot)].fill(0xff);
        Ok(())
    }
    fn program_control(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ProgramControl) {
            return Err(Fault::ProgramControl);
        }
        let torn = self.fail(Fault::TornProgramControl);
        let page = &mut self.control[Self::slot(slot)];
        page.fill(0xff);
        page[..record.len()].copy_from_slice(record);
        if torn {
            page[0] ^= 1;
        }
        Ok(())
    }
    fn read_pending(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ReadPending)
            || self.fail(Fault::VerifyPending) && self.pending_reads >= 2
            || self.fail(Fault::VerifyPendingB)
                && self.pending_reads >= 2
                && matches!(slot, Slot::B)
        {
            return Err(if self.fault == Fault::ReadPending {
                Fault::ReadPending
            } else if self.fault == Fault::VerifyPendingB {
                Fault::VerifyPendingB
            } else {
                Fault::VerifyPending
            });
        }
        out.copy_from_slice(&self.pending[Self::slot(slot)]);
        self.pending_reads += 1;
        Ok(())
    }
    fn erase_pending(&mut self, slot: Slot) -> Result<(), Self::Error> {
        if self.fail(Fault::ErasePending) {
            return Err(Fault::ErasePending);
        }
        if self.fail(Fault::ErasePendingB) && matches!(slot, Slot::B) {
            return Err(Fault::ErasePendingB);
        }
        self.pending[Self::slot(slot)].fill(0xff);
        Ok(())
    }
    fn program_pending(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ProgramPending) {
            return Err(Fault::ProgramPending);
        }
        let torn = self.fail(Fault::TornProgramPending);
        let page = &mut self.pending[Self::slot(slot)];
        page.fill(0xff);
        page[..record.len()].copy_from_slice(record);
        if torn {
            page[0] ^= 1;
        }
        Ok(())
    }
}

fn run_with<R>(
    store: &mut Store,
    f: impl FnOnce(&mut Store, &mut FirstWriteScratch<'_>) -> R,
) -> R {
    let mut control_a = [0; PAGE];
    let mut control_b = [0; PAGE];
    let mut pending_a = [0; PAGE];
    let mut pending_b = [0; PAGE];
    let mut body = [0; radio_hand::control::MAX_DURABLE_BODY];
    let mut page = [0; PAGE];
    let mut readback = [0; PAGE];
    f(
        store,
        &mut FirstWriteScratch::new(
            &mut control_a,
            &mut control_b,
            &mut pending_a,
            &mut pending_b,
            &mut body,
            &mut page,
            &mut readback,
        )
        .unwrap(),
    )
}

fn staged_store() -> Store {
    let mut store = Store::blank();
    assert_eq!(
        run_with(&mut store, |store, scratch| stage_first_write(
            store,
            scratch,
            &state(),
            NODE,
            &facts()
        )),
        Ok(StageOutcome::Staged)
    );
    store
}

#[test]
fn stage_failures_never_touch_control_and_leave_a_safe_retry_state() {
    for fault in [
        Fault::ErasePending,
        Fault::ProgramPending,
        Fault::VerifyPending,
        Fault::TornProgramPending,
    ] {
        let mut store = Store::blank();
        store.inject(fault);
        assert!(
            run_with(&mut store, |store, scratch| stage_first_write(
                store,
                scratch,
                &state(),
                NODE,
                &facts()
            ))
            .is_err()
        );
        let status = first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts(),
        );
        assert_eq!(status.control, PairEvidence::Blank);
        assert!(status.claim_eligible() || status.resume_eligible() || status.abandon_eligible());
    }
}

#[test]
fn inspection_read_failures_are_typed_and_scratch_refuses_mismatched_slots() {
    for fault in [Fault::ReadControl, Fault::ReadPending] {
        let mut store = Store::blank();
        store.inject(fault);
        assert!(matches!(
            run_with(&mut store, |store, scratch| stage_first_write(
                store,
                scratch,
                &state(),
                NODE,
                &facts()
            )),
            Err(FirstWriteStorageError::Store { .. })
        ));
    }
    let mut a = [0; 8];
    let mut b = [0; 8];
    let mut p_a = [0; 8];
    let mut p_b = [0; 8];
    let mut body = [0; radio_hand::control::MAX_DURABLE_BODY];
    let mut page = [0; 7];
    let mut readback = [0; 8];
    assert!(
        FirstWriteScratch::new(
            &mut a,
            &mut b,
            &mut p_a,
            &mut p_b,
            &mut body,
            &mut page,
            &mut readback
        )
        .is_err()
    );
}

#[test]
fn corrupt_control_can_be_repaired_from_valid_pending_but_torn_control_never_hides_pending() {
    let mut store = staged_store();
    store.control[0][0] = 0;
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .control,
        PairEvidence::Valid
    );

    let mut store = staged_store();
    store.inject(Fault::TornProgramControl);
    assert!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Corrupt);
    assert_eq!(status.pending, PairEvidence::Valid);
}

#[test]
fn corrupt_repair_advances_outer_sequence_and_handles_max_without_losing_pending() {
    let mut store = staged_store();
    radio_hand::store::encode(41, b"malformed-durable-body", &mut store.control[1]).unwrap();
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Corrupt);
    assert_eq!(status.pending, PairEvidence::Valid);
    store.inject(Fault::ProgramControl);
    assert!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Corrupt);
    assert_eq!(status.pending, PairEvidence::Valid);
    store.inject(Fault::None);
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    assert_eq!(
        radio_hand::store::decode(&store.control[0])
            .unwrap()
            .sequence,
        42
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .pending,
        PairEvidence::Blank
    );

    let mut store = staged_store();
    radio_hand::store::encode(u32::MAX, b"malformed-durable-a", &mut store.control[0]).unwrap();
    radio_hand::store::encode(u32::MAX, b"malformed-durable-b", &mut store.control[1]).unwrap();
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    assert_eq!(
        radio_hand::store::decode(&store.control[0])
            .unwrap()
            .sequence,
        u32::MAX
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .control,
        PairEvidence::Valid
    );
}

#[test]
fn resume_failures_preserve_pending_until_control_is_durable_then_report_cleanup() {
    for fault in [Fault::EraseControl, Fault::ProgramControl] {
        let mut store = staged_store();
        store.inject(fault);
        assert!(
            run_with(&mut store, |store, scratch| resume_first_write(
                store,
                scratch,
                NODE,
                &facts()
            ))
            .is_err()
        );
        let status = first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts(),
        );
        assert_eq!(status.control, PairEvidence::Blank);
        assert_eq!(status.pending, PairEvidence::Valid);
    }
    let mut store = staged_store();
    store.inject(Fault::VerifyControl);
    assert!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Valid);
    assert_eq!(status.pending, PairEvidence::Valid);

    for fault in [
        Fault::ErasePending,
        Fault::VerifyPending,
        Fault::ErasePendingB,
        Fault::VerifyPendingB,
    ] {
        let mut store = staged_store();
        store.inject(fault);
        let outcome = run_with(&mut store, |store, scratch| {
            resume_first_write(store, scratch, NODE, &facts())
        })
        .unwrap();
        assert!(matches!(
            outcome,
            ResumeOutcome::CommittedWithCleanupFailure(FirstWriteStorageError::Store { .. })
        ));
        let status = first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts(),
        );
        assert_eq!(status.control, PairEvidence::Valid);
    }
}

#[test]
fn abandon_requires_blank_control_and_its_partial_cleanup_is_safe_to_retry() {
    let mut store = staged_store();
    store.inject(Fault::ErasePending);
    assert!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .pending,
        PairEvidence::Valid
    );
    store.inject(Fault::None);
    assert_eq!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(AbandonOutcome::Abandoned)
    );
    let blank = [0xff; PAGE];
    assert!(
        first_write_status(
            &blank,
            &blank,
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .ordinary_service_eligible()
    );
}

#[test]
fn abandon_erases_corrupt_pending_only_with_blank_control_and_can_retry_after_slot_b_failure() {
    let mut store = staged_store();
    store.pending[0][0] = 0;
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .actions()
        .bits(),
        FirstWriteActions::ABANDON
    );
    assert_eq!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(AbandonOutcome::Abandoned)
    );

    let mut store = staged_store();
    store.inject(Fault::ErasePendingB);
    assert!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Blank);
    assert_eq!(status.pending, PairEvidence::Blank);
    store.inject(Fault::None);
    assert_eq!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(AbandonOutcome::NothingStaged)
    );
}

#[test]
fn valid_control_wins_stale_pending_and_abandon_will_not_erase_it() {
    let mut store = staged_store();
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    // Restage a valid pending record synthetically to prove it cannot supersede control.
    let stale = staged_store();
    store.pending = stale.pending;
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::AlreadyControlPresent)
    );
    assert!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
}
