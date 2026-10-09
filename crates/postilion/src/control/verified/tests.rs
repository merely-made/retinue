use std::time::Duration;

use heapless::Vec;
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use seneschal::control::{
    BoardRecoveryFacts, COMMIT_TOKEN_LEN, ChangeId, CommitArguments, ConfigGeneration,
    ControlStatusAuthority, ControlStatusV1, ControllerRole, DurableConfig, DurableState,
    FirstWriteStatus, MAX_CONTROL_COMMAND_FRAME_LEN, MAX_CONTROL_RESPONSE_FRAME_LEN,
    ManagementCarrier, ManagementCarrierSet, NodeId, Operation, OwnerGrant, PairEvidence,
    ProvisionalApplyArguments, PublicConfigurationV1, RecoveryClause, RecoveryPathFacts,
    RecoveryPolicy, Refusal, Request, Response, ResponseBody, ReticulumTransportPolicy,
    decode_command_frame, decode_verified_command, encode_response_frame, restore_control_verifier,
};
use seneschal::region::Region;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::super::sign_request;
use super::*;

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
fn board_answer(state: &mut DurableState, frame: &[u8], authority_diagnostic: bool) -> Response {
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
    let mut bytes = [0_u8; seneschal::control::CONTROL_STATUS_V1_LEN];
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

/// A board that runs the real durable model for the lifecycle, minus flash and radio.
fn board_lifecycle(
    state: &mut DurableState,
    frame: &[u8],
    token: [u8; COMMIT_TOKEN_LEN],
) -> Response {
    use seneschal::control::{ChangeId, SemanticTagKey};
    let mut verifier = restore_control_verifier(state).unwrap();
    let command = decode_command_frame(frame).unwrap();
    let verified = verifier.verify(command).unwrap();
    let inbound = decode_verified_command(&verified).unwrap();
    state
        .advance_verified_outer_counter(inbound.verified_controller(), inbound.counter())
        .unwrap();
    let request = inbound.request();
    let key = SemanticTagKey::from_bytes([0x80; 32]);
    let facts = BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    match request.operation {
        Operation::ProvisionalApply => {
            let arguments = ProvisionalApplyArguments::decode(&request.arguments).unwrap();
            state
                .arm_with_facts(
                    NODE,
                    inbound.verified_controller(),
                    request,
                    &key,
                    &facts,
                    arguments.change,
                    DurableConfig {
                        public: arguments.public,
                        sealed_credentials: Vec::new(),
                    },
                    1_000,
                    1_000 + arguments.lifetime_ms,
                    token,
                    Vec::new(),
                )
                .unwrap()
                .into_response()
        }
        Operation::Commit => {
            let arguments = CommitArguments::decode(&request.arguments).unwrap();
            state
                .commit(
                    NODE,
                    inbound.verified_controller(),
                    request,
                    &key,
                    arguments.change,
                    arguments.candidate_generation,
                    arguments.commit_token,
                    2_000,
                )
                .unwrap()
                .into_response()
        }
        _ => {
            let _ = ChangeId([0; 16]);
            panic!("the lifecycle fake serves apply and commit only")
        }
    }
}

#[tokio::test]
async fn provisional_apply_then_commit_moves_known_good() {
    let owner = owner();
    let mut state = state(&owner);
    let (client, mut board) = tokio::io::duplex(2048);
    let board_task = tokio::spawn(async move {
        let frame = read_one_frame(&mut board).await;
        let response = board_lifecycle(&mut state, &frame, [0x5c; COMMIT_TOKEN_LEN]);
        write_response(&mut board, &response).await;
        let frame = read_one_frame(&mut board).await;
        let response = board_lifecycle(&mut state, &frame, [0; COMMIT_TOKEN_LEN]);
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
    let candidate = PublicConfigurationV1::new(
        Region::Us915,
        selvage::PhyProfile::meshtastic_long_fast(908_125_000),
        ReticulumTransportPolicy::new(false, false, 0).unwrap(),
        ManagementCarrierSet::from_mask(1).unwrap(),
    )
    .unwrap();
    let mutation = Mutation {
        sequence: 1,
        expected_generation: ConfigGeneration(3),
    };
    let provisional = controller
        .provisional_apply(1, mutation, ChangeId([0x31; 16]), candidate, 60_000)
        .await
        .unwrap();
    assert_eq!(provisional.candidate_generation, ConfigGeneration(4));
    assert_eq!(provisional.deadline_ms, 61_000);
    assert_eq!(provisional.commit_token, [0x5c; COMMIT_TOKEN_LEN]);
    assert!(!format!("{provisional:?}").contains("5c"));

    let committed = controller
        .commit(
            2,
            Mutation {
                sequence: 2,
                expected_generation: ConfigGeneration(3),
            },
            &provisional,
        )
        .await
        .unwrap();
    assert_eq!(committed.known_good_generation, ConfigGeneration(4));
    let state = board_task.await.unwrap();
    assert_eq!(state.known_good().generation, ConfigGeneration(4));
    assert_eq!(state.known_good().configuration.public, candidate);
    assert!(state.provisional().is_none());
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
