use heapless::Vec;
use radio_hand::control::{
    BoardRecoveryFacts, COMMAND_OPCODE, ConfigGeneration, ControllerId, ControllerRole,
    DurableConfig, DurableState, InboundControlError, ManagementCarrier, ManagementCarrierSet,
    NodeId, Operation, OwnerGrant, PublicConfigurationV1, RecoveryClause, RecoveryPathFacts,
    RecoveryPolicy, Request, ReticulumTransportPolicy, TransactionId, VerifiedCounterError,
    VerifierRestoreError, decode_verified_command, restore_verifier,
};
use radio_hand::region::Region;
use retinue::command::{Command, TargetClass, Verifier};
use retinue::hash::AddressHash;
use retinue::identity::{IDENTITY_LEN, PrivateIdentity};

fn operator(fill: u8) -> PrivateIdentity {
    let mut secret = [0u8; IDENTITY_LEN];
    secret[..32].fill(fill);
    secret[32..].fill(fill ^ 0xff);
    PrivateIdentity::from_secret_bytes(&secret)
}

fn node() -> AddressHash {
    AddressHash::from_bytes([0x10; 16])
}

fn request(payload: &[u8]) -> Request {
    Request {
        transaction: TransactionId([0x30; 16]),
        transaction_sequence: 9,
        expected_generation: ConfigGeneration(7),
        operation: Operation::StageConfiguration,
        arguments: Vec::try_from(payload).unwrap(),
    }
}

fn request_payload(request: &Request) -> Vec<u8, { radio_hand::control::MAX_REQUEST_LEN }> {
    let mut out = [0; radio_hand::control::MAX_REQUEST_LEN];
    let length = radio_hand::control::encode_request(request, &mut out).unwrap();
    Vec::try_from(&out[..length]).unwrap()
}

fn verifier(operator: &PrivateIdentity) -> Verifier<2> {
    let mut verifier = Verifier::new(node());
    verifier.authorize(*operator.public()).unwrap();
    verifier
}

fn durable_configuration() -> DurableConfig {
    DurableConfig {
        public: PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap(),
        sealed_credentials: Vec::new(),
    }
}

fn recovery_policy() -> RecoveryPolicy {
    RecoveryPolicy::new(
        RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
        RecoveryClause::disabled(),
    )
    .unwrap()
}
fn recovery_facts() -> BoardRecoveryFacts {
    BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn verified_node_command_enters_control_with_authenticated_metadata() {
    let operator = operator(0x11);
    let request = request(b"wifi configuration");
    let payload = request_payload(&request);
    let wire = Command {
        key_id: operator.hash(),
        class: TargetClass::Node,
        target: node(),
        counter: 7,
        opcode: COMMAND_OPCODE,
        payload: &payload,
    }
    .sign(&operator)
    .unwrap();
    let mut verifier = verifier(&operator);
    let verified = verifier.verify(&wire).unwrap();

    let inbound = decode_verified_command(&verified).unwrap();
    assert_eq!(inbound.node(), NodeId(*node().as_bytes()));
    assert_eq!(
        inbound.controller_id(),
        ControllerId(*operator.hash().as_bytes())
    );
    assert_eq!(inbound.counter(), 7);
    assert_eq!(inbound.request(), &request);
    assert_eq!(verifier.accepted(), 1);
}

#[test]
fn fleet_commands_are_not_control_node_authority() {
    let operator = operator(0x11);
    let fleet = AddressHash::from_bytes([0xf1; 16]);
    let payload = request_payload(&request(b"fleet"));
    let wire = Command {
        key_id: operator.hash(),
        class: TargetClass::Fleet,
        target: fleet,
        counter: 1,
        opcode: COMMAND_OPCODE,
        payload: &payload,
    }
    .sign(&operator)
    .unwrap();
    let mut verifier = verifier(&operator);
    verifier.join_fleet(fleet);
    let verified = verifier.verify(&wire).unwrap();

    assert!(matches!(
        decode_verified_command(&verified),
        Err(InboundControlError::NonNodeTarget)
    ));
}

#[test]
fn another_verified_application_opcode_is_refused() {
    let operator = operator(0x11);
    let payload = request_payload(&request(b"wrong opcode"));
    let wire = Command {
        key_id: operator.hash(),
        class: TargetClass::Node,
        target: node(),
        counter: 1,
        opcode: COMMAND_OPCODE.wrapping_add(1),
        payload: &payload,
    }
    .sign(&operator)
    .unwrap();
    let mut verifier = verifier(&operator);
    let verified = verifier.verify(&wire).unwrap();

    assert!(matches!(
        decode_verified_command(&verified),
        Err(InboundControlError::WrongOpcode(_))
    ));
}

#[test]
fn verified_but_malformed_control_payload_is_refused() {
    let operator = operator(0x11);
    let wire = Command {
        key_id: operator.hash(),
        class: TargetClass::Node,
        target: node(),
        counter: 1,
        opcode: COMMAND_OPCODE,
        payload: b"not a WN0 request",
    }
    .sign(&operator)
    .unwrap();
    let mut verifier = verifier(&operator);
    let verified = verifier.verify(&wire).unwrap();

    assert!(matches!(
        decode_verified_command(&verified),
        Err(InboundControlError::InvalidRequest(_))
    ));
}

#[test]
fn inbound_control_debug_redacts_request_arguments() {
    let operator = operator(0x11);
    let payload = request_payload(&request(b"SUPER_SECRET_WIFI_PASSWORD_7f2c"));
    let wire = Command {
        key_id: operator.hash(),
        class: TargetClass::Node,
        target: node(),
        counter: 1,
        opcode: COMMAND_OPCODE,
        payload: &payload,
    }
    .sign(&operator)
    .unwrap();
    let mut verifier = verifier(&operator);
    let verified = verifier.verify(&wire).unwrap();
    let rendered = format!("{:?}", decode_verified_command(&verified).unwrap());

    assert!(!rendered.contains("SUPER_SECRET_WIFI_PASSWORD_7f2c"));
    assert!(rendered.contains("InboundControl"));
    assert!(rendered.contains("arguments_len"));
    assert!(rendered.contains("operation"));
}

#[test]
fn durable_grants_restore_a_fail_closed_verifier_and_its_counter() {
    let operator = operator(0x11);
    let mut state = DurableState::new(
        NodeId(*node().as_bytes()),
        Vec::from_slice(&[OwnerGrant::from_retinue_identity(
            operator.public(),
            ControllerRole::Owner,
        )])
        .unwrap(),
        ConfigGeneration(0),
        durable_configuration(),
        recovery_policy(),
        &recovery_facts(),
    )
    .unwrap();
    let verified = decode_verified_command(
        &verifier(&operator)
            .verify(
                &Command {
                    key_id: operator.hash(),
                    class: TargetClass::Node,
                    target: node(),
                    counter: 7,
                    opcode: COMMAND_OPCODE,
                    payload: &request_payload(&request(b"counter")),
                }
                .sign(&operator)
                .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    state
        .advance_verified_outer_counter(verified.verified_controller(), verified.counter())
        .unwrap();
    let mut restored = restore_verifier::<1>(&state).unwrap();
    let stale = Command {
        key_id: operator.hash(),
        class: TargetClass::Node,
        target: node(),
        counter: 7,
        opcode: COMMAND_OPCODE,
        payload: &request_payload(&request(b"counter")),
    }
    .sign(&operator)
    .unwrap();
    assert!(restored.verify(&stale).is_err());
    let next = Command {
        key_id: operator.hash(),
        class: TargetClass::Node,
        target: node(),
        counter: 8,
        opcode: COMMAND_OPCODE,
        payload: &request_payload(&request(b"counter")),
    }
    .sign(&operator)
    .unwrap();
    assert!(restored.verify(&next).is_ok());
    assert_eq!(
        state.advance_verified_outer_counter(verified.verified_controller(), 7),
        Err(VerifiedCounterError::NotMonotonic)
    );
}

#[test]
fn verifier_restore_rejects_capacity() {
    let operator = operator(0x11);
    let valid = DurableState::new(
        NodeId(*node().as_bytes()),
        Vec::from_slice(&[OwnerGrant::from_retinue_identity(
            operator.public(),
            ControllerRole::Owner,
        )])
        .unwrap(),
        ConfigGeneration(0),
        durable_configuration(),
        recovery_policy(),
        &recovery_facts(),
    )
    .unwrap();
    assert!(matches!(
        restore_verifier::<0>(&valid),
        Err(VerifierRestoreError::Capacity)
    ));
}
