//! Test authority minted the only way a board may: a real Retinue signature, verified.

use retinue::command::{Command, TargetClass, Verifier};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;

use super::VerifiedController;

/// The controller `signer` becomes once one signed command from it passes the verifier.
pub(crate) fn verified_controller(signer: &PrivateIdentity) -> VerifiedController {
    let node = AddressHash::from_bytes([0; 16]);
    let wire = Command {
        key_id: signer.hash(),
        class: TargetClass::Node,
        target: node,
        counter: 1,
        opcode: 0,
        payload: &[],
    }
    .sign(signer)
    .unwrap();
    let mut verifier = Verifier::<1>::new(node);
    verifier.authorize(*signer.public()).unwrap();
    VerifiedController::from_verified_command(&verifier.verify(&wire).unwrap())
}
