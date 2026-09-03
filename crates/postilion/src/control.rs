//! Host-side WN0 control adapter.
//!
//! Postilion owns carriage orchestration; the signed command envelope remains
//! [`retinue::command`], and the bounded semantic payload remains
//! [`radio_hand::control`]. The WN0 helpers below deliberately have no carrier or UI state;
//! [`first_owner`] separately owns its bounded physical-presence USB adapter.

use radio_hand::control::{self, Request, Response};
use retinue::command::{Command, TargetClass};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;

/// The local, physical-presence first-owner carrier and controller flow.
pub mod first_owner;
/// The ordinary-runtime, diagnostic-only control-status carrier.
pub mod status;
/// The ordinary-runtime, controller-authenticated control carrier.
pub mod verified;

/// Errors at the Postilion/control boundary.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("control request is too large")]
    RequestTooLarge,
    #[error("command signing refused: {0:?}")]
    Signing(retinue::command::Refusal),
    #[error("control response has the wrong opcode")]
    WrongOpcode,
    #[error("control response is addressed to a different node")]
    ResponseTargetMismatch,
    #[error("control response belongs to a different transaction")]
    ResponseTransactionMismatch,
    #[error("invalid control response: {0:?}")]
    InvalidResponse(control::DecodeError),
}

/// Encode and sign one WN0 request for a caller-selected node and replay counter.
pub fn sign_request(
    request: &Request,
    signer: &PrivateIdentity,
    target: AddressHash,
    counter: u64,
) -> Result<Vec<u8>, Error> {
    let mut payload = [0_u8; control::MAX_REQUEST_LEN];
    let length =
        control::encode_request(request, &mut payload).map_err(|_| Error::RequestTooLarge)?;
    let command = Command {
        key_id: signer.hash(),
        class: TargetClass::Node,
        target,
        counter,
        opcode: control::COMMAND_OPCODE,
        payload: &payload[..length],
    };
    command
        .sign(signer)
        .map(|bytes| bytes.to_vec())
        .map_err(Error::Signing)
}

/// Verify the WN0 opcode and decode a response payload received from a carrier.
pub fn decode_response_payload(
    opcode: u8,
    expected_node: AddressHash,
    expected_transaction: control::TransactionId,
    payload: &[u8],
) -> Result<Response, Error> {
    if opcode != control::COMMAND_OPCODE {
        return Err(Error::WrongOpcode);
    }
    let response = control::decode_response(payload).map_err(Error::InvalidResponse)?;
    if response.node.0 != *expected_node.as_bytes() {
        return Err(Error::ResponseTargetMismatch);
    }
    if response.transaction != expected_transaction {
        return Err(Error::ResponseTransactionMismatch);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use radio_hand::control::{self, ConfigGeneration, Operation, TransactionId};
    use retinue::command::{Refusal, Verifier};

    fn identity(byte: u8) -> PrivateIdentity {
        PrivateIdentity::from_secret_bytes(&[byte; 64])
    }

    fn request() -> Request {
        control::decode_request(&control::GOLDEN_REQUEST).unwrap()
    }

    #[test]
    fn golden_vectors_are_consumed_at_the_host_boundary() {
        let request = request();
        assert_eq!(request.transaction, TransactionId([0x30; 16]));
        assert_eq!(request.expected_generation, ConfigGeneration(7));
        assert_eq!(request.operation, Operation::StageConfiguration);
        let response = control::decode_response(&control::GOLDEN_RESPONSE).unwrap();
        assert_eq!(response.known_good_generation, ConfigGeneration(7));
        assert_eq!(response.effective_generation, Some(ConfigGeneration(8)));
        assert_eq!(
            decode_response_payload(
                control::COMMAND_OPCODE,
                AddressHash::from_bytes([0x10; 16]),
                TransactionId([0x30; 16]),
                &control::GOLDEN_RESPONSE,
            )
            .unwrap(),
            response
        );
    }

    #[test]
    fn signed_request_is_verified_once_and_exact_replay_is_refused() {
        let signer = identity(0x42);
        let target = AddressHash::from_bytes([0x11; 16]);
        let request = request();
        let first = sign_request(&request, &signer, target, 1).unwrap();
        let second = sign_request(&request, &signer, target, 2).unwrap();
        let mut verifier = Verifier::<2>::new(target);
        verifier.authorize(*signer.public()).unwrap();
        let command = verifier.accept(&first).unwrap();
        assert_eq!(command.opcode, control::COMMAND_OPCODE);
        assert_eq!(verifier.accept(&first), Err(Refusal::CounterReplayed));
        assert!(
            verifier.accept(&second).is_ok(),
            "same semantic transaction may use a fresh counter"
        );
    }

    #[test]
    fn response_decode_fails_closed_on_wrong_opcode_and_invalid_bytes() {
        assert_eq!(
            decode_response_payload(
                control::COMMAND_OPCODE.wrapping_add(1),
                AddressHash::from_bytes([0x10; 16]),
                TransactionId([0x30; 16]),
                &control::GOLDEN_RESPONSE
            ),
            Err(Error::WrongOpcode)
        );
        assert!(matches!(
            decode_response_payload(
                control::COMMAND_OPCODE,
                AddressHash::from_bytes([0x10; 16]),
                TransactionId([0x30; 16]),
                &control::GOLDEN_RESPONSE[..10],
            ),
            Err(Error::InvalidResponse(_))
        ));
        assert_eq!(
            decode_response_payload(
                control::COMMAND_OPCODE,
                AddressHash::from_bytes([0x11; 16]),
                TransactionId([0x30; 16]),
                &control::GOLDEN_RESPONSE,
            ),
            Err(Error::ResponseTargetMismatch)
        );
        assert_eq!(
            decode_response_payload(
                control::COMMAND_OPCODE,
                AddressHash::from_bytes([0x10; 16]),
                TransactionId([0x31; 16]),
                &control::GOLDEN_RESPONSE,
            ),
            Err(Error::ResponseTransactionMismatch)
        );
    }
}
