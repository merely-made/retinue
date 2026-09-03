//! Async, board-neutral ordering for durable WN1 configuration transitions.
//!
//! Any storage or recovery uncertainty poisons this runtime. A [`BootState::Blank`] result is
//! only the fact that both A/B records are erased: it never authorizes ownership establishment;
//! firmware must require physical presence and a separate commissioning marker. In particular, an outer Retinue
//! verifier may have advanced before its accepted counter is journaled; discard that verifier and
//! rebuild it from the durable grants/counters before accepting another envelope.
use super::*;
use crate::store::Slot;
use core::{convert::Infallible, fmt};
use heapless::Vec;
mod quiet;
use quiet::ActiveQuietGuard;
pub use quiet::{LiveOutcome, QuietExit, QuietGuard, QuietWindow};
#[cfg(feature = "control-retinue")]
mod inbound;
pub const MIN_DURABLE_SLOT_BYTES: usize = crate::store::encoded_len(MAX_DURABLE_BODY);
/// Longest a controller may leave a candidate unconfirmed. A longer request is refused as
/// invalid arguments; the bound keeps a lost controller from parking a node on a candidate.
pub const MAX_PROVISIONAL_LIFETIME_MS: u64 = 10 * 60 * 1_000;
/// Shortest useful lifetime: below this a commit cannot arrive over any real carrier.
pub const MIN_PROVISIONAL_LIFETIME_MS: u64 = 1_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableScratchError {
    SlotTooSmall { available: usize, required: usize },
    UnequalSlotLengths { slot_a: usize, slot_b: usize },
    PageLengthMismatch { slot: usize, page: usize },
}
pub struct DurableScratch<'a> {
    slot_a: &'a mut [u8],
    slot_b: &'a mut [u8],
    body: &'a mut [u8; MAX_DURABLE_BODY],
    page: &'a mut [u8],
}
impl<'a> DurableScratch<'a> {
    pub fn new(
        slot_a: &'a mut [u8],
        slot_b: &'a mut [u8],
        body: &'a mut [u8; MAX_DURABLE_BODY],
        page: &'a mut [u8],
    ) -> Result<Self, DurableScratchError> {
        if slot_a.len() < MIN_DURABLE_SLOT_BYTES {
            return Err(DurableScratchError::SlotTooSmall {
                available: slot_a.len(),
                required: MIN_DURABLE_SLOT_BYTES,
            });
        }
        if slot_b.len() < MIN_DURABLE_SLOT_BYTES {
            return Err(DurableScratchError::SlotTooSmall {
                available: slot_b.len(),
                required: MIN_DURABLE_SLOT_BYTES,
            });
        }
        if slot_a.len() != slot_b.len() {
            return Err(DurableScratchError::UnequalSlotLengths {
                slot_a: slot_a.len(),
                slot_b: slot_b.len(),
            });
        }
        if page.len() != slot_a.len() {
            return Err(DurableScratchError::PageLengthMismatch {
                slot: slot_a.len(),
                page: page.len(),
            });
        }
        Ok(Self {
            slot_a,
            slot_b,
            body,
            page,
        })
    }
}
/// Applies a sealed durable configuration without blocking the executive.
///
/// Returning an error means the live configuration is uncertain; the runtime restores
/// known-good or poisons itself when that recovery cannot be established. The
/// board applier is the trusted regulatory boundary: it must apply
/// [`PublicConfigurationV1::effective_reticulum_phy`] with its hardware ceiling,
/// or route the requested profile through `Executive::apply_profile`.
/// It must never pass the requested profile directly to lower-level radio service.
#[allow(async_fn_in_trait)]
pub trait ConfigApplier {
    type Error;
    async fn apply(&mut self, configuration: &DurableConfig) -> Result<(), Self::Error>;
}
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedProvisional {
    pub change: ChangeId,
    pub candidate: DurableConfig,
    pub deadline_ms: u64,
    pub commit_token: [u8; COMMIT_TOKEN_LEN],
    pub result: Vec<u8, MAX_RESULT>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedCommit {
    pub change: ChangeId,
    pub candidate_generation: ConfigGeneration,
    pub commit_token: [u8; COMMIT_TOKEN_LEN],
}
impl fmt::Debug for PreparedProvisional {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedProvisional")
            .field("change", &self.change)
            .field("candidate", &self.candidate)
            .field("deadline_ms", &self.deadline_ms)
            .field("commit_token", &"[redacted]")
            .field("result_len", &self.result.len())
            .finish()
    }
}
impl fmt::Debug for PreparedCommit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedCommit")
            .field("change", &self.change)
            .field("candidate_generation", &self.candidate_generation)
            .field("commit_token", &"[redacted]")
            .finish()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootState {
    Ready,
    Blank,
}
pub enum RuntimeError<S, A = Infallible, Q = Infallible> {
    NoDurableState,
    Poisoned,
    ResetPending,
    QuietInProgress,
    BootAlreadyAttempted,
    BootIncomplete,
    ForeignNode { expected: NodeId, found: NodeId },
    Refused(Refusal),
    VerifiedCounter(VerifiedCounterError),
    Load(DurableLoadError),
    Durable(DurableError),
    Store(S),
    Apply(A),
    Quiet(Q),
    ReadbackMismatch,
}
impl<S, Q> RuntimeError<S, Infallible, Q> {
    /// Re-types an error from a path that cannot fail to apply into a path that can.
    pub fn widen_apply<A>(self) -> RuntimeError<S, A, Q> {
        match self {
            Self::NoDurableState => RuntimeError::NoDurableState,
            Self::Poisoned => RuntimeError::Poisoned,
            Self::ResetPending => RuntimeError::ResetPending,
            Self::QuietInProgress => RuntimeError::QuietInProgress,
            Self::BootAlreadyAttempted => RuntimeError::BootAlreadyAttempted,
            Self::BootIncomplete => RuntimeError::BootIncomplete,
            Self::ForeignNode { expected, found } => RuntimeError::ForeignNode { expected, found },
            Self::Refused(r) => RuntimeError::Refused(r),
            Self::VerifiedCounter(e) => RuntimeError::VerifiedCounter(e),
            Self::Load(e) => RuntimeError::Load(e),
            Self::Durable(e) => RuntimeError::Durable(e),
            Self::Store(e) => RuntimeError::Store(e),
            Self::Apply(never) => match never {},
            Self::Quiet(e) => RuntimeError::Quiet(e),
            Self::ReadbackMismatch => RuntimeError::ReadbackMismatch,
        }
    }
}
impl<S, A, Q> fmt::Debug for RuntimeError<S, A, Q> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDurableState => f.write_str("RuntimeError::NoDurableState"),
            Self::Poisoned => f.write_str("RuntimeError::Poisoned"),
            Self::ResetPending => f.write_str("RuntimeError::ResetPending"),
            Self::QuietInProgress => f.write_str("RuntimeError::QuietInProgress"),
            Self::BootAlreadyAttempted => f.write_str("RuntimeError::BootAlreadyAttempted"),
            Self::BootIncomplete => f.write_str("RuntimeError::BootIncomplete"),
            Self::ForeignNode { expected, found } => f
                .debug_struct("RuntimeError::ForeignNode")
                .field("expected", expected)
                .field("found", found)
                .finish(),
            Self::Refused(x) => f.debug_tuple("RuntimeError::Refused").field(x).finish(),
            Self::VerifiedCounter(x) => f
                .debug_tuple("RuntimeError::VerifiedCounter")
                .field(x)
                .finish(),
            Self::Load(x) => f.debug_tuple("RuntimeError::Load").field(x).finish(),
            Self::Durable(x) => f.debug_tuple("RuntimeError::Durable").field(x).finish(),
            Self::Store(_) => f.write_str("RuntimeError::Store([redacted])"),
            Self::Apply(_) => f.write_str("RuntimeError::Apply([redacted])"),
            Self::Quiet(_) => f.write_str("RuntimeError::Quiet([redacted])"),
            Self::ReadbackMismatch => f.write_str("RuntimeError::ReadbackMismatch"),
        }
    }
}
pub struct ControlRuntime {
    expected_node: NodeId,
    recovery_facts: BoardRecoveryFacts,
    state: Option<DurableState>,
    semantic_tag_key: SemanticTagKey,
    poisoned: bool,
    reset_pending: bool,
    quiet_in_progress: bool,
    boot_attempted: bool,
    boot_completed: bool,
    recovered_rollback: bool,
}

struct SplitBoot<'a, S, A> {
    store: &'a mut S,
    applier: &'a mut A,
}

impl<S, A> AbSlotStore for SplitBoot<'_, S, A>
where
    S: AbSlotStore,
{
    type Error = S::Error;

    fn read_slot(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        self.store.read_slot(slot, out)
    }

    fn erase_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
        self.store.erase_slot(slot)
    }

    fn program_slot(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        self.store.program_slot(slot, record)
    }
}

impl<S, A> ConfigApplier for SplitBoot<'_, S, A>
where
    A: ConfigApplier,
{
    type Error = A::Error;

    async fn apply(&mut self, configuration: &DurableConfig) -> Result<(), Self::Error> {
        self.applier.apply(configuration).await
    }
}

impl ControlRuntime {
    /// Creates a runtime after the board has completed a real hardware reset.
    ///
    /// # Safety
    /// Call only once after actual hardware reset from the board startup owner; calling without
    /// reset acknowledges pending quiet work and can resume an unsafe radio or flash state.
    #[allow(unsafe_code)]
    pub unsafe fn new_after_hardware_reset(
        expected_node: NodeId,
        semantic_tag_key: SemanticTagKey,
        recovery_facts: BoardRecoveryFacts,
    ) -> Self {
        Self {
            expected_node,
            recovery_facts,
            state: None,
            semantic_tag_key,
            poisoned: false,
            reset_pending: false,
            quiet_in_progress: false,
            boot_attempted: false,
            boot_completed: false,
            recovered_rollback: false,
        }
    }
    pub const fn state(&self) -> Option<&DurableState> {
        self.state.as_ref()
    }
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    pub const fn reset_pending(&self) -> bool {
        self.reset_pending
    }
    pub const fn quiet_in_progress(&self) -> bool {
        self.quiet_in_progress
    }
    /// Whether this successful boot recovered a durable provisional candidate
    /// to known-good before ordinary service was permitted.
    pub const fn recovered_rollback(&self) -> bool {
        self.recovered_rollback
    }
    /// The board time at which the armed candidate, if any, rolls back on its own. A
    /// board loop uses it to schedule [`Self::expire`] instead of polling flash.
    pub fn provisional_deadline_ms(&self) -> Option<u64> {
        self.state
            .as_ref()?
            .provisional()
            .map(Provisional::deadline_ms)
    }
    fn poison<S, A, Q>(&mut self, e: RuntimeError<S, A, Q>) -> RuntimeError<S, A, Q> {
        self.poisoned = true;
        e
    }
    fn ready<S, A, Q>(&self) -> Result<(), RuntimeError<S, A, Q>> {
        if self.poisoned {
            Err(RuntimeError::Poisoned)
        } else if self.reset_pending {
            Err(RuntimeError::ResetPending)
        } else if self.quiet_in_progress {
            Err(RuntimeError::QuietInProgress)
        } else if !self.boot_completed {
            Err(RuntimeError::BootIncomplete)
        } else if self.state.is_none() {
            Err(RuntimeError::NoDurableState)
        } else {
            Ok(())
        }
    }
    fn read<S, A, Q>(
        &self,
        s: &mut S,
        x: &mut DurableScratch<'_>,
    ) -> Result<(), RuntimeError<S::Error, A, Q>>
    where
        S: AbSlotStore,
    {
        s.read_slot(Slot::A, x.slot_a)
            .map_err(RuntimeError::Store)?;
        s.read_slot(Slot::B, x.slot_b).map_err(RuntimeError::Store)
    }
    fn persist<S, A, Q>(
        &mut self,
        s: &mut S,
        x: &mut DurableScratch<'_>,
    ) -> Result<(), RuntimeError<S::Error, A, Q>>
    where
        S: AbSlotStore,
    {
        self.read(s, x)?;
        let w = next_record(
            x.slot_a,
            x.slot_b,
            self.state.as_ref().unwrap(),
            x.body,
            x.page,
        )
        .map_err(RuntimeError::Durable)?;
        s.erase_slot(w.slot).map_err(RuntimeError::Store)?;
        s.program_slot(w.slot, &x.page[..w.len])
            .map_err(RuntimeError::Store)?;
        match w.slot {
            Slot::A => s
                .read_slot(Slot::A, x.slot_a)
                .map_err(RuntimeError::Store)?,
            Slot::B => s
                .read_slot(Slot::B, x.slot_b)
                .map_err(RuntimeError::Store)?,
        };
        if load(x.slot_a, x.slot_b).ok().as_ref() != self.state.as_ref() {
            return Err(RuntimeError::ReadbackMismatch);
        }
        Ok(())
    }
    fn complete_live<T, S, A, Q>(
        &mut self,
        result: Result<T, RuntimeError<S, A, Q>>,
        finish: Result<QuietExit, Q>,
    ) -> Result<LiveOutcome<T>, RuntimeError<S, A, Q>> {
        match finish {
            Err(error) => {
                self.poisoned = true;
                // A lost quiet exit is always fatal. If the operation also failed, return the
                // operation error so callers do not lose the first actionable fault.
                match result {
                    Err(original) => Err(original),
                    Ok(_) => Err(RuntimeError::Quiet(error)),
                }
            }
            Ok(exit) => {
                self.quiet_in_progress = false;
                if exit == QuietExit::ResetRequired {
                    self.reset_pending = true;
                }
                match result {
                    Ok(value) => Ok(LiveOutcome { value, exit }),
                    Err(error) => {
                        if !matches!(error, RuntimeError::Refused(_) | RuntimeError::Apply(_)) {
                            self.poisoned = true;
                        }
                        Err(error)
                    }
                }
            }
        }
    }
    async fn enter_live<'a, S, A, Q>(
        &mut self,
        q: &'a mut Q,
    ) -> Result<ActiveQuietGuard<Q::Guard<'a>>, RuntimeError<S, A, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.quiet_in_progress = true;
        match q.enter().await {
            Ok(guard) => Ok(ActiveQuietGuard::new(guard)),
            Err(error) => {
                // An entry error is retryable only when the board contract guarantees that
                // stopping never began. A post-stop failure must be represented by a pending
                // future or a returned guard whose Drop path aborts the quiet window.
                self.quiet_in_progress = false;
                Err(RuntimeError::Quiet(error))
            }
        }
    }
    /// Loads durable state before radio services are armed.
    ///
    /// This one-shot pre-radio path needs no [`QuietWindow`]; repeat, reset-pending, or
    /// abandoned-quiet calls are refused before storage or application work.
    pub async fn boot_pre_radio<S, A>(
        &mut self,
        s: &mut S,
        a: &mut A,
        x: &mut DurableScratch<'_>,
    ) -> Result<BootState, RuntimeError<S::Error, A::Error>>
    where
        S: AbSlotStore,
        A: ConfigApplier,
    {
        let mut owner = SplitBoot {
            store: s,
            applier: a,
        };
        self.boot_pre_radio_owner(&mut owner, x).await
    }

    /// Loads durable state through one owner that can provide both storage and application.
    ///
    /// Firmware uses this form when the radio, flash store, and configuration application share
    /// one exclusive board owner. The split [`Self::boot_pre_radio`] form remains for hosts and
    /// boards whose two adapters are independently borrowable.
    pub async fn boot_pre_radio_owner<B>(
        &mut self,
        owner: &mut B,
        x: &mut DurableScratch<'_>,
    ) -> Result<BootState, RuntimeError<<B as AbSlotStore>::Error, <B as ConfigApplier>::Error>>
    where
        B: AbSlotStore + ConfigApplier,
    {
        if self.poisoned {
            return Err(RuntimeError::Poisoned);
        }
        if self.reset_pending {
            return Err(RuntimeError::ResetPending);
        }
        if self.quiet_in_progress {
            return Err(RuntimeError::QuietInProgress);
        }
        if self.boot_attempted {
            return Err(RuntimeError::BootAlreadyAttempted);
        }
        self.boot_attempted = true;
        if let Err(e) = self.read(owner, x) {
            return Err(self.poison(e));
        };
        let mut state = match load(x.slot_a, x.slot_b) {
            Ok(v) => v,
            Err(DurableLoadError::Blank) => {
                self.state = None;
                self.boot_completed = false;
                return Ok(BootState::Blank);
            }
            Err(e) => return Err(self.poison(RuntimeError::Load(e))),
        };
        if state.node() != self.expected_node {
            return Err(self.poison(RuntimeError::ForeignNode {
                expected: self.expected_node,
                found: state.node(),
            }));
        };
        if let Err(e) = state.validate_recovery_facts(&self.recovery_facts) {
            return Err(self.poison(RuntimeError::Durable(e)));
        }
        let r = state.recover_after_reboot();
        self.recovered_rollback = matches!(r, Recovery::Rollback { .. });
        let c = match &r {
            Recovery::None => state.known_good().configuration.clone(),
            Recovery::Rollback { configuration } => configuration.clone(),
        };
        self.state = Some(state);
        if let Err(e) = owner.apply(&c).await {
            return Err(self.poison(RuntimeError::Apply(e)));
        }
        if matches!(r, Recovery::Rollback { .. })
            && let Err(e) = self.persist(owner, x)
        {
            return Err(self.poison(e));
        }
        self.boot_completed = true;
        Ok(BootState::Ready)
    }
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
    async fn restore<B, Q>(
        &mut self,
        owner: &mut B,
        x: &mut DurableScratch<'_>,
    ) -> Result<bool, RuntimeError<<B as AbSlotStore>::Error, <B as ConfigApplier>::Error, Q>>
    where
        B: AbSlotStore + ConfigApplier,
    {
        let r = self.state.as_mut().unwrap().rollback();
        self.recover(owner, x, r).await
    }
    async fn recover<B, Q>(
        &mut self,
        owner: &mut B,
        x: &mut DurableScratch<'_>,
        r: Recovery,
    ) -> Result<bool, RuntimeError<<B as AbSlotStore>::Error, <B as ConfigApplier>::Error, Q>>
    where
        B: AbSlotStore + ConfigApplier,
    {
        let Recovery::Rollback { configuration } = r else {
            return Ok(false);
        };
        if let Err(e) = owner.apply(&configuration).await {
            return Err(self.poison(RuntimeError::Apply(e)));
        }
        if let Err(e) = self.persist(owner, x) {
            return Err(self.poison(e));
        }
        Ok(true)
    }
    pub async fn expire<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
        now: u64,
    ) -> Result<LiveOutcome<bool>, RuntimeError<Q::StoreError, Q::ApplyError, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = async {
            let r = self.state.as_mut().unwrap().expire(now);
            self.recover(guard.inner_mut(), x, r).await
        }
        .await;
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
    pub async fn revert<Q>(
        &mut self,
        q: &mut Q,
        x: &mut DurableScratch<'_>,
    ) -> Result<LiveOutcome<bool>, RuntimeError<Q::StoreError, Q::ApplyError, Q::Error>>
    where
        Q: QuietWindow,
    {
        self.ready()?;
        let mut guard = self.enter_live(q).await?;
        let result = self.restore(guard.inner_mut(), x).await;
        let finish = guard.finish().await;
        self.complete_live(result, finish)
    }
}
#[cfg(test)]
mod tests;
