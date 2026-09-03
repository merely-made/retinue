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
