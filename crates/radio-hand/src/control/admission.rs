use core::fmt;

#[cfg(test)]
use core::fmt::Write;

#[cfg(test)]
use heapless::String;
use heapless::Vec;

use super::model::*;

#[derive(Clone, PartialEq, Eq)]
struct RequestIdentity {
    controller: ControllerId,
    transaction: TransactionId,
    expected_generation: ConfigGeneration,
    operation: Operation,
    arguments: Vec<u8, MAX_ARGUMENTS>,
}
impl fmt::Debug for RequestIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestIdentity")
            .field("controller", &self.controller)
            .field("transaction", &self.transaction)
            .field("expected_generation", &self.expected_generation)
            .field("operation", &self.operation)
            .field("arguments_len", &self.arguments.len())
            .finish()
    }
}
impl RequestIdentity {
    fn from_request(controller: ControllerId, request: &Request) -> Self {
        Self {
            controller,
            transaction: request.transaction,
            expected_generation: request.expected_generation,
            operation: request.operation,
            arguments: request.arguments.clone(),
        }
    }
}

/// Result of volatile request admission. A caller returns its cached response for `Duplicate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Fresh,
    Duplicate,
    Refused(Refusal),
}

/// RAM-only duplicate guard. WN1 must replace its response cache and ledger with durable state.
#[derive(Clone)]
pub struct RequestAdmission<const N: usize> {
    entries: [Option<RequestIdentity>; N],
    next: usize,
}
impl<const N: usize> fmt::Debug for RequestAdmission<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestAdmission")
            .field("entries", &self.entries.iter().flatten().count())
            .field("next", &self.next)
            .finish()
    }
}
impl<const N: usize> RequestAdmission<N> {
    pub fn new() -> Self {
        Self {
            entries: core::array::from_fn(|_| None),
            next: 0,
        }
    }
    pub fn admit(
        &mut self,
        controller: VerifiedController,
        current_generation: ConfigGeneration,
        request: &Request,
    ) -> Admission {
        let identity = RequestIdentity::from_request(controller.0, request);
        for entry in self.entries.iter().flatten() {
            if entry.controller == identity.controller && entry.transaction == identity.transaction
            {
                return if entry == &identity {
                    Admission::Duplicate
                } else {
                    Admission::Refused(Refusal::TransactionConflict)
                };
            }
        }
        if request.operation.requires_generation()
            && request.expected_generation != current_generation
        {
            return Admission::Refused(Refusal::StaleGeneration);
        }
        if N == 0 {
            return Admission::Refused(Refusal::Capacity);
        }
        self.entries[self.next] = Some(identity);
        self.next = (self.next + 1) % N;
        Admission::Fresh
    }
}
impl<const N: usize> Default for RequestAdmission<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(operation: Operation, tx: u8) -> Request {
        Request {
            transaction: TransactionId([tx; ID_LEN]),
            transaction_sequence: u64::from(tx),
            expected_generation: ConfigGeneration(7),
            operation,
            arguments: Vec::new(),
        }
    }
    fn verified(byte: u8) -> VerifiedController {
        VerifiedController::from_verified_key(ControllerId([byte; ID_LEN]))
    }
    #[test]
    fn idempotence_is_scoped_by_verified_controller_and_read_only_requests_ignore_cas() {
        let mut admission = RequestAdmission::<8>::new();
        let mutation = request(Operation::StageConfiguration, 0x30);
        assert_eq!(
            admission.admit(verified(1), ConfigGeneration(7), &mutation),
            Admission::Fresh
        );
        assert_eq!(
            admission.admit(verified(1), ConfigGeneration(8), &mutation),
            Admission::Duplicate
        );
        let mut conflict = mutation.clone();
        conflict.operation = Operation::Reboot;
        assert_eq!(
            admission.admit(verified(1), ConfigGeneration(8), &conflict),
            Admission::Refused(Refusal::TransactionConflict)
        );
        assert_eq!(
            admission.admit(verified(2), ConfigGeneration(7), &mutation),
            Admission::Fresh
        );
        for (index, operation) in [
            Operation::Capabilities,
            Operation::Status,
            Operation::WifiScan,
            Operation::RecoveryStatus,
        ]
        .into_iter()
        .enumerate()
        {
            let read = request(operation, 0x40 + index as u8);
            assert!(!operation.requires_generation());
            assert_eq!(
                admission.admit(verified(1), ConfigGeneration(8), &read),
                Admission::Fresh
            );
        }
        assert_eq!(
            admission.admit(
                verified(1),
                ConfigGeneration(8),
                &request(Operation::Commit, 0x50)
            ),
            Admission::Refused(Refusal::StaleGeneration)
        );
    }

    #[test]
    fn admission_debug_redacts_arguments() {
        let mut admission = RequestAdmission::<1>::new();
        let mut request = request(Operation::Commit, 9);
        request
            .arguments
            .extend_from_slice(b"wifi-secret-marker")
            .unwrap();
        assert_eq!(
            admission.admit(verified(1), ConfigGeneration(7), &request),
            Admission::Fresh
        );
        let mut rendered = String::<128>::new();
        write!(&mut rendered, "{admission:?}").unwrap();
        assert!(!rendered.contains("wifi-secret-marker"));
    }
}
