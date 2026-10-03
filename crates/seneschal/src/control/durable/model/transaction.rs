use heapless::Vec;

use super::*;

impl DurableState {
    /// Arms and returns a provisional result. Persist `self` before applying `candidate`; a
    /// power cut at any later point rolls back to known-good.
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub fn arm(
        &mut self,
        node: NodeId,
        controller: VerifiedController,
        request: &Request,
        semantic_tag_key: &SemanticTagKey,
        change: ChangeId,
        candidate: DurableConfig,
        now_ms: u64,
        deadline_ms: u64,
        commit_token: [u8; COMMIT_TOKEN_LEN],
        result: Vec<u8, MAX_RESULT>,
    ) -> Result<Transition, Refusal> {
        self.arm_inner(
            node,
            controller,
            request,
            semantic_tag_key,
            None,
            change,
            candidate,
            now_ms,
            deadline_ms,
            commit_token,
            result,
        )
    }

    /// Runtime admission variant. Firmware supplies immutable board facts before a
    /// candidate can be journaled or applied.
    #[allow(clippy::too_many_arguments)]
    pub fn arm_with_facts(
        &mut self,
        node: NodeId,
        controller: VerifiedController,
        request: &Request,
        semantic_tag_key: &SemanticTagKey,
        facts: &BoardRecoveryFacts,
        change: ChangeId,
        candidate: DurableConfig,
        now_ms: u64,
        deadline_ms: u64,
        commit_token: [u8; COMMIT_TOKEN_LEN],
        result: Vec<u8, MAX_RESULT>,
    ) -> Result<Transition, Refusal> {
        self.arm_inner(
            node,
            controller,
            request,
            semantic_tag_key,
            Some(facts),
            change,
            candidate,
            now_ms,
            deadline_ms,
            commit_token,
            result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn arm_inner(
        &mut self,
        node: NodeId,
        controller: VerifiedController,
        request: &Request,
        semantic_tag_key: &SemanticTagKey,
        facts: Option<&BoardRecoveryFacts>,
        change: ChangeId,
        candidate: DurableConfig,
        now_ms: u64,
        deadline_ms: u64,
        commit_token: [u8; COMMIT_TOKEN_LEN],
        result: Vec<u8, MAX_RESULT>,
    ) -> Result<Transition, Refusal> {
        if node != self.node {
            return Err(Refusal::WrongNode);
        }
        let tag = SemanticTag::derive(semantic_tag_key, node, controller, request);
        let controller = controller.0;
        if let Some(response) = self.admit_mutation(controller, request, tag)? {
            return Ok(Transition::replayed(response));
        }
        if !self.permits_configuration(controller, &candidate) {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::Unauthorized));
        }
        if request.operation != Operation::ProvisionalApply {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::UnsupportedOperation));
        }
        if request.expected_generation != self.known_good.generation {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::StaleGeneration));
        }
        if deadline_ms <= now_ms {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::TransactionExpired));
        }
        if !self.recovery_policy.configuration_satisfies(&candidate)
            || facts.is_some_and(|facts| {
                recovery::validate_policy_candidate(self.recovery_policy, &candidate, facts)
                    .is_err()
            })
        {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::UnsafeRecoveryPath));
        }
        if self.provisional.is_some() {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::Busy));
        }
        let candidate_generation = match self.generation_watermark.checked_successor() {
            Ok(generation) => generation,
            Err(reason) => return Ok(self.cache_refusal(controller, request, tag, reason)),
        };
        self.generation_watermark = candidate_generation;
        self.provisional = Some(Provisional {
            controller,
            change,
            semantic: SemanticKey::from_request(request, tag),
            candidate_generation,
            candidate,
            deadline_ms,
            commit_token,
            result: result.clone(),
        });
        Ok(Transition::changed(self.provisional_response()))
    }

    /// Commits the exact armed transaction. Firmware has applied the candidate only after
    /// journaling `arm`; this method changes the durable state alone.
    #[allow(clippy::too_many_arguments)]
    pub fn commit(
        &mut self,
        node: NodeId,
        controller: VerifiedController,
        request: &Request,
        semantic_tag_key: &SemanticTagKey,
        change: ChangeId,
        candidate_generation: ConfigGeneration,
        commit_token: [u8; COMMIT_TOKEN_LEN],
        now_ms: u64,
    ) -> Result<Transition, Refusal> {
        self.validate_semantics().map_err(|_| Refusal::Internal)?;
        if node != self.node {
            return Err(Refusal::WrongNode);
        }
        let tag = SemanticTag::derive(semantic_tag_key, node, controller, request);
        let controller = controller.0;
        if let Some(response) = self.admit_mutation(controller, request, tag)? {
            return Ok(Transition::replayed(response));
        }
        if !self.permits_provisional_commit(controller) {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::Unauthorized));
        }
        if request.operation != Operation::Commit {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::UnsupportedOperation));
        }
        if request.expected_generation != self.known_good.generation {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::StaleGeneration));
        }
        let Some(provisional) = self.provisional.as_ref() else {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::InvalidCommit));
        };
        let valid = provisional.controller == controller
            && provisional.change == change
            && provisional.candidate_generation == candidate_generation
            && provisional.commit_token == commit_token;
        if !valid {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::InvalidCommit));
        }
        if now_ms >= provisional.deadline_ms {
            return Ok(self.cache_refusal(controller, request, tag, Refusal::TransactionExpired));
        }
        let provisional = self.provisional.take().expect("checked above");
        self.known_good = KnownGood {
            generation: provisional.candidate_generation,
            configuration: provisional.candidate,
        };
        self.receipt = Some(CachedReceipt {
            controller,
            semantic: SemanticKey::from_request(request, tag),
            body: ReceiptBody::Applied {
                known_good_generation: self.known_good.generation,
                result: provisional.result.clone(),
            },
        });
        Ok(Transition::changed(Response {
            node: self.node,
            transaction: request.transaction,
            known_good_generation: self.known_good.generation,
            effective_generation: Some(self.known_good.generation),
            body: ResponseBody::Applied(provisional.result),
        }))
    }
}
