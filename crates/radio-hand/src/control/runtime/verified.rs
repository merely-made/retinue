//! Verified-command operations: arm, commit, refuse, revert, status, and outer-counter journaling.

use super::*;

impl ControlRuntime {
    #[allow(clippy::too_many_arguments)]
    pub async fn arm<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        node: NodeId,
        c: VerifiedController,
        outer: u64,
        r: &Request,
        now: u64,
        p: PreparedProvisional,
    ) -> Result<LiveOutcome<Transition>, RuntimeError<Q::StoreError, Q::ApplyError, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = async {
            self.state
                .as_mut()
                .unwrap()
                .advance_verified_outer_counter(c, outer)
                .map_err(|e| self.poison(RuntimeError::VerifiedCounter(e)))?;
            let t = self.state.as_mut().unwrap().arm_with_facts(
                node,
                c,
                r,
                &self.semantic_tag_key,
                &self.recovery_facts,
                p.change,
                p.candidate.clone(),
                now,
                p.deadline_ms,
                p.commit_token,
                p.result,
            );
            self.persist(guard.inner_mut(), x)?;
            let t = t.map_err(RuntimeError::Refused)?;
            if t.is_changed()
                && matches!(t.response().body, ResponseBody::Provisional { .. })
                && let Err(e) = guard.inner_mut().apply(&p.candidate).await
            {
                let err = RuntimeError::Apply(e);
                self.restore(guard.inner_mut(), x).await?;
                return Err(err);
            }
            Ok(t)
        }
        .await;
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn commit<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        node: NodeId,
        c: VerifiedController,
        outer: u64,
        r: &Request,
        now: u64,
        p: PreparedCommit,
    ) -> Result<LiveOutcome<Transition>, RuntimeError<Q::StoreError, Infallible, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = (|| {
            self.state
                .as_mut()
                .unwrap()
                .advance_verified_outer_counter(c, outer)
                .map_err(|e| self.poison(RuntimeError::VerifiedCounter(e)))?;
            let t = self.state.as_mut().unwrap().commit(
                node,
                c,
                r,
                &self.semantic_tag_key,
                p.change,
                p.candidate_generation,
                p.commit_token,
                now,
            );
            self.persist(guard.inner_mut(), x)?;
            t.map_err(RuntimeError::Refused)
        })();
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
    /// Journals a verified command's outer counter and answers it with a refusal.
    ///
    /// For a verified command whose arguments the board cannot use: the counter must still
    /// become durable so the same envelope can never be replayed, and the controller learns
    /// why nothing changed.
    pub async fn refuse_verified<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        c: VerifiedController,
        outer: u64,
        r: &Request,
        reason: Refusal,
    ) -> Result<LiveOutcome<Response>, RuntimeError<Q::StoreError, Infallible, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = (|| {
            self.state
                .as_mut()
                .unwrap()
                .advance_verified_outer_counter(c, outer)
                .map_err(|e| self.poison(RuntimeError::VerifiedCounter(e)))?;
            self.persist(guard.inner_mut(), x)?;
            let state = self.state.as_ref().unwrap();
            Ok(Response {
                node: state.node(),
                transaction: r.transaction,
                known_good_generation: state.known_good().generation,
                effective_generation: None,
                body: ResponseBody::Refused {
                    reason,
                    result: Vec::new(),
                },
            })
        })();
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
    /// Abandons the armed candidate named by `change` and restores known-good now.
    ///
    /// The outer counter is journaled first. A controller without commit rights, or a
    /// change id that does not name the armed candidate, is answered with a refusal after
    /// that journaling; nothing else moves. Otherwise the rollback is journaled, known-good
    /// is re-applied to the hardware, and the response reports the restored generation.
    #[allow(clippy::too_many_arguments)]
    pub async fn revert_verified<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        node: NodeId,
        c: VerifiedController,
        outer: u64,
        r: &Request,
        change: ChangeId,
    ) -> Result<LiveOutcome<Response>, RuntimeError<Q::StoreError, Q::ApplyError, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = async {
            self.state
                .as_mut()
                .unwrap()
                .advance_verified_outer_counter(c, outer)
                .map_err(|e| self.poison(RuntimeError::VerifiedCounter(e)))?;
            let (own_node, known_good, permitted, names_armed) = {
                let state = self.state.as_ref().unwrap();
                (
                    state.node(),
                    state.known_good().generation,
                    state.permits_provisional_revert(c),
                    state.provisional().is_some_and(|p| p.change() == change),
                )
            };
            if node != own_node {
                return Err(RuntimeError::Refused(Refusal::WrongNode));
            }
            let refused = |reason| Response {
                node: own_node,
                transaction: r.transaction,
                known_good_generation: known_good,
                effective_generation: None,
                body: ResponseBody::Refused {
                    reason,
                    result: Vec::new(),
                },
            };
            if !permitted {
                self.persist(guard.inner_mut(), x)?;
                return Ok(refused(Refusal::Unauthorized));
            }
            if !names_armed {
                self.persist(guard.inner_mut(), x)?;
                return Ok(refused(Refusal::InvalidCommit));
            }
            self.restore(guard.inner_mut(), x).await?;
            Ok(Response {
                node: own_node,
                transaction: r.transaction,
                known_good_generation: known_good,
                effective_generation: Some(known_good),
                body: ResponseBody::Applied(Vec::new()),
            })
        }
        .await;
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
    /// Answers one verified read-only request with the public control status.
    ///
    /// The accepted outer counter becomes durable inside the quiet window before any response
    /// exists, exactly as for a mutation: a Status the board answered but did not journal would
    /// be replayable after reboot. Only [`Operation::Status`] is observed; every other verified
    /// operation is refused as unsupported after its counter is journaled, so a slice that
    /// implements mutations must route them before falling back here. The body is the fixed
    /// public status payload, bound to the request transaction with `VerifiedController`
    /// authority; it never includes configuration, grants, receipts, or secrets.
    #[allow(clippy::too_many_arguments)]
    pub async fn observe_status<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        node: NodeId,
        c: VerifiedController,
        outer: u64,
        r: &Request,
        first_write: FirstWriteStatus,
    ) -> Result<LiveOutcome<Response>, RuntimeError<Q::StoreError, Infallible, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = (|| {
            self.state
                .as_mut()
                .unwrap()
                .advance_verified_outer_counter(c, outer)
                .map_err(|e| self.poison(RuntimeError::VerifiedCounter(e)))?;
            self.persist(guard.inner_mut(), x)?;
            let state = self.state.as_ref().unwrap();
            if node != state.node() {
                return Err(RuntimeError::Refused(Refusal::WrongNode));
            }
            let body = if r.operation == Operation::Status {
                let status = ControlStatusV1::for_verified_controller(
                    first_write,
                    state,
                    self.recovered_rollback,
                    r.transaction,
                );
                let mut bytes = [0_u8; CONTROL_STATUS_V1_LEN];
                status
                    .encode(&mut bytes)
                    .map_err(|_| RuntimeError::Refused(Refusal::Internal))?;
                ResponseBody::Observed(
                    Vec::from_slice(&bytes)
                        .map_err(|_| RuntimeError::Refused(Refusal::Internal))?,
                )
            } else {
                ResponseBody::Refused {
                    reason: Refusal::UnsupportedOperation,
                    result: Vec::new(),
                }
            };
            Ok(Response {
                node: state.node(),
                transaction: r.transaction,
                known_good_generation: state.known_good().generation,
                effective_generation: None,
                body,
            })
        })();
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
    pub async fn record_verified_outer<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        c: VerifiedController,
        outer: u64,
    ) -> Result<LiveOutcome<()>, RuntimeError<Q::StoreError, Infallible, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = (|| {
            self.state
                .as_mut()
                .unwrap()
                .advance_verified_outer_counter(c, outer)
                .map_err(|e| self.poison(RuntimeError::VerifiedCounter(e)))?;
            self.persist(guard.inner_mut(), x)
        })();
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
}
