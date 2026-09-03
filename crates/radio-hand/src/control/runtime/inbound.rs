use super::*;

#[cfg(feature = "control-retinue")]
impl ControlRuntime {
    #[allow(clippy::too_many_arguments)]
    pub async fn arm_inbound<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        inbound: &InboundControl,
        now: u64,
        p: PreparedProvisional,
    ) -> Result<LiveOutcome<Transition>, RuntimeError<Q::StoreError, Q::ApplyError, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.arm(
            q,
            x,
            inbound.node(),
            inbound.verified_controller(),
            inbound.counter(),
            inbound.request(),
            now,
            p,
        )
        .await
    }

    pub async fn commit_inbound<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        inbound: &InboundControl,
        now: u64,
        p: PreparedCommit,
    ) -> Result<LiveOutcome<Transition>, RuntimeError<Q::StoreError, Infallible, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.commit(
            q,
            x,
            inbound.node(),
            inbound.verified_controller(),
            inbound.counter(),
            inbound.request(),
            now,
            p,
        )
        .await
    }

    /// [`Self::observe_status`] for a verified inbound control request.
    pub async fn observe_status_inbound<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        inbound: &InboundControl,
        first_write: FirstWriteStatus,
    ) -> Result<LiveOutcome<Response>, RuntimeError<Q::StoreError, Infallible, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.observe_status(
            q,
            x,
            inbound.node(),
            inbound.verified_controller(),
            inbound.counter(),
            inbound.request(),
            first_write,
        )
        .await
    }

    /// Serves one verified inbound request through the lifecycle this slice supports.
    ///
    /// `Status` is observed. `ProvisionalApply` stages the carried public configuration
    /// with empty sealed credentials as the candidate, applies it, and answers with the
    /// board-minted `commit_token`; the caller supplies that token from true entropy.
    /// `Commit` and `Revert` name the armed candidate by the controller's change id. Every
    /// other operation, and every malformed argument body, is refused after the outer
    /// counter is journaled. `now_ms` is board time; the candidate's deadline is
    /// `now_ms + lifetime_ms`, with the lifetime bounded by the runtime's constants.
    #[allow(clippy::too_many_arguments)]
    pub async fn serve_inbound<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        inbound: &InboundControl,
        now_ms: u64,
        first_write: FirstWriteStatus,
        commit_token: [u8; COMMIT_TOKEN_LEN],
    ) -> Result<LiveOutcome<Response>, RuntimeError<Q::StoreError, Q::ApplyError, Q::Error>>
    where
        Q: QuietWindow,
    {
        let request = inbound.request();
        match request.operation {
            Operation::ProvisionalApply => {
                let arguments = match ProvisionalApplyArguments::decode(&request.arguments) {
                    Ok(arguments)
                        if (MIN_PROVISIONAL_LIFETIME_MS..=MAX_PROVISIONAL_LIFETIME_MS)
                            .contains(&arguments.lifetime_ms) =>
                    {
                        arguments
                    }
                    _ => {
                        return self
                            .refuse_inbound(q, x, inbound, Refusal::InvalidArguments)
                            .await;
                    }
                };
                let prepared = PreparedProvisional {
                    change: arguments.change,
                    candidate: DurableConfig {
                        public: arguments.public,
                        sealed_credentials: Vec::new(),
                    },
                    deadline_ms: now_ms.saturating_add(arguments.lifetime_ms),
                    commit_token,
                    result: Vec::new(),
                };
                self.arm_inbound(q, x, inbound, now_ms, prepared)
                    .await
                    .map(|outcome| outcome.map(Transition::into_response))
            }
            Operation::Commit => {
                let Ok(arguments) = CommitArguments::decode(&request.arguments) else {
                    return self
                        .refuse_inbound(q, x, inbound, Refusal::InvalidArguments)
                        .await;
                };
                let prepared = PreparedCommit {
                    change: arguments.change,
                    candidate_generation: arguments.candidate_generation,
                    commit_token: arguments.commit_token,
                };
                self.commit_inbound(q, x, inbound, now_ms, prepared)
                    .await
                    .map(|outcome| outcome.map(Transition::into_response))
                    .map_err(RuntimeError::widen_apply)
            }
            Operation::Revert => {
                let Ok(arguments) = RevertArguments::decode(&request.arguments) else {
                    return self
                        .refuse_inbound(q, x, inbound, Refusal::InvalidArguments)
                        .await;
                };
                self.revert_verified(
                    q,
                    x,
                    inbound.node(),
                    inbound.verified_controller(),
                    inbound.counter(),
                    request,
                    arguments.change,
                )
                .await
            }
            // `observe_status` answers Status and refuses everything else as unsupported.
            _ => self
                .observe_status_inbound(q, x, inbound, first_write)
                .await
                .map_err(RuntimeError::widen_apply),
        }
    }

    async fn refuse_inbound<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        inbound: &InboundControl,
        reason: Refusal,
    ) -> Result<LiveOutcome<Response>, RuntimeError<Q::StoreError, Q::ApplyError, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.refuse_verified(
            q,
            x,
            inbound.verified_controller(),
            inbound.counter(),
            inbound.request(),
            reason,
        )
        .await
        .map_err(RuntimeError::widen_apply)
    }

    pub async fn record_verified_command<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        v: &retinue::command::VerifiedCommand<'_>,
    ) -> Result<LiveOutcome<()>, RuntimeError<Q::StoreError, Infallible, Q::Error>>
    where
        Q: QuietWindow,
    {
        // Call this after an already-verified envelope is refused by WN0 decoding (wrong
        // opcode, non-node target, or malformed payload), so the outer counter is durable
        // before the caller rebuilds or reuses its Retinue verifier.
        let controller = ControllerId(*v.key_id().as_bytes());
        self.record_verified_outer(
            q,
            x,
            VerifiedController::from_verified_key(controller),
            v.counter(),
        )
        .await
    }
}
