//! Authorization, durable-state invariants, and replay admission for [`DurableState`].

use heapless::Vec;
use sha2::{Digest, Sha256};

use super::super::super::{
    ControllerId, ControllerRole, Operation, Refusal, Request, Response, ResponseBody,
    TransactionId, validate_retinue_public_identity,
};
use super::{
    CachedReceipt, DurableConfig, DurableError, DurableState, MUTATION_SEQUENCE_WINDOW,
    ReceiptBody, SemanticKey, SemanticTag, Transition,
};

impl DurableState {
    pub(super) fn permits_configuration(
        &self,
        controller: ControllerId,
        candidate: &DurableConfig,
    ) -> bool {
        self.owner_grants.iter().any(|grant| {
            grant.controller == controller
                && match grant.role {
                    ControllerRole::Owner => true,
                    ControllerRole::Operator => {
                        candidate.public.region() == self.known_good.configuration.public.region()
                            && candidate.public.enabled_management_carriers()
                                == self
                                    .known_good
                                    .configuration
                                    .public
                                    .enabled_management_carriers()
                            && candidate.sealed_credentials
                                == self.known_good.configuration.sealed_credentials
                    }
                    ControllerRole::Observer | ControllerRole::Updater => false,
                }
        })
    }

    pub(super) fn permits_provisional_commit(&self, controller: ControllerId) -> bool {
        self.owner_grants.iter().any(|grant| {
            grant.controller == controller
                && matches!(grant.role, ControllerRole::Operator | ControllerRole::Owner)
        })
    }

    /// The invariant shared by construction, durable decoding, and commit.
    /// Treat every violation as corrupt durable state rather than repairing it.
    pub(super) fn validate_semantics(&self) -> Result<(), DurableError> {
        if self.recovery_policy.validate_structure().is_err()
            || !self
                .recovery_policy
                .configuration_satisfies(&self.known_good.configuration)
            || self.generation_watermark < self.known_good.generation
        {
            return Err(DurableError::Malformed);
        }
        let mut has_owner = false;
        for (index, grant) in self.owner_grants.iter().enumerate() {
            validate_retinue_public_identity(&grant.retinue_public_identity)
                .map_err(|_| DurableError::Malformed)?;
            let digest = Sha256::digest(grant.retinue_public_identity);
            if digest[..16] != grant.controller.0 {
                return Err(DurableError::Malformed);
            }
            if matches!(grant.role, ControllerRole::Owner) {
                has_owner = true;
            }
            if self.owner_grants[..index]
                .iter()
                .any(|prior| prior.controller == grant.controller)
            {
                return Err(DurableError::Malformed);
            }
        }
        if !has_owner {
            return Err(DurableError::Malformed);
        }
        if let Some(provisional) = &self.provisional
            && (!self
                .recovery_policy
                .configuration_satisfies(&provisional.candidate)
                || provisional.candidate_generation <= self.known_good.generation
                || provisional.candidate_generation != self.generation_watermark
                || provisional.semantic.operation != Operation::ProvisionalApply
                || !self.permits_configuration(provisional.controller, &provisional.candidate)
                || self
                    .owner_grants
                    .iter()
                    .find(|grant| grant.controller == provisional.controller)
                    .is_none_or(|grant| {
                        provisional.semantic.transaction_sequence > grant.accepted_mutation_sequence
                    }))
        {
            return Err(DurableError::Malformed);
        }
        if let Some(CachedReceipt {
            body:
                ReceiptBody::Applied {
                    known_good_generation,
                    ..
                },
            ..
        }) = &self.receipt
            && *known_good_generation != self.known_good.generation
        {
            return Err(DurableError::Malformed);
        }
        if let Some(receipt) = &self.receipt
            && self
                .owner_grants
                .iter()
                .find(|grant| grant.controller == receipt.controller)
                .is_none_or(|grant| {
                    receipt.semantic.transaction_sequence > grant.accepted_mutation_sequence
                })
        {
            return Err(DurableError::Malformed);
        }
        Ok(())
    }

    pub(super) fn admit_mutation(
        &mut self,
        controller: ControllerId,
        request: &Request,
        semantic_tag: SemanticTag,
    ) -> Result<Option<Response>, Refusal> {
        if let Some(response) = self.replay(controller, request, semantic_tag)? {
            return Ok(Some(response));
        }
        let grant = self
            .owner_grants
            .iter_mut()
            .find(|grant| grant.controller == controller)
            .ok_or(Refusal::Unauthorized)?;
        if request.transaction_sequence <= grant.accepted_mutation_sequence {
            return Err(Refusal::TransactionExpired);
        }
        if request.transaction_sequence - grant.accepted_mutation_sequence
            > MUTATION_SEQUENCE_WINDOW
        {
            return Err(Refusal::TransactionTooFar);
        }
        grant.accepted_mutation_sequence = request.transaction_sequence;
        Ok(None)
    }

    pub(super) fn cache_refusal(
        &mut self,
        controller: ControllerId,
        request: &Request,
        semantic_tag: SemanticTag,
        reason: Refusal,
    ) -> Transition {
        self.receipt = Some(CachedReceipt {
            controller,
            semantic: SemanticKey::from_request(request, semantic_tag),
            body: ReceiptBody::Refused(reason),
        });
        Transition::changed(self.refusal_response(request.transaction, reason))
    }

    fn replay(
        &self,
        controller: ControllerId,
        request: &Request,
        semantic_tag: SemanticTag,
    ) -> Result<Option<Response>, Refusal> {
        if let Some(provisional) = self.provisional.as_ref()
            && provisional.controller == controller
            && provisional.semantic.transaction_sequence == request.transaction_sequence
        {
            return if provisional.semantic.matches(request, semantic_tag) {
                Ok(Some(self.provisional_response()))
            } else {
                Err(Refusal::TransactionConflict)
            };
        }
        let Some(receipt) = self.receipt.as_ref() else {
            return Ok(None);
        };
        if receipt.controller != controller
            || receipt.semantic.transaction_sequence != request.transaction_sequence
        {
            return Ok(None);
        }
        if !receipt.semantic.matches(request, semantic_tag) {
            return Err(Refusal::TransactionConflict);
        }
        Ok(Some(match &receipt.body {
            ReceiptBody::Applied {
                known_good_generation,
                result,
            } => Response {
                node: self.node,
                transaction: request.transaction,
                known_good_generation: *known_good_generation,
                effective_generation: Some(*known_good_generation),
                body: ResponseBody::Applied(result.clone()),
            },
            ReceiptBody::Refused(reason) => self.refusal_response(request.transaction, *reason),
        }))
    }

    pub(super) fn provisional_response(&self) -> Response {
        let provisional = self
            .provisional
            .as_ref()
            .expect("an armed transaction exists");
        Response {
            node: self.node,
            transaction: provisional.semantic.transaction,
            known_good_generation: self.known_good.generation,
            effective_generation: Some(provisional.candidate_generation),
            body: ResponseBody::Provisional {
                deadline_ms: provisional.deadline_ms,
                commit_token: provisional.commit_token,
                result: provisional.result.clone(),
            },
        }
    }

    fn refusal_response(&self, transaction: TransactionId, reason: Refusal) -> Response {
        Response {
            node: self.node,
            transaction,
            known_good_generation: self.known_good.generation,
            effective_generation: None,
            body: ResponseBody::Refused {
                reason,
                result: Vec::new(),
            },
        }
    }
}
