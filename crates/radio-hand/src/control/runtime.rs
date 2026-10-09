//! Async, board-neutral ordering for durable WN1 configuration transitions.
//!
//! Any storage or recovery uncertainty poisons this runtime. [`BootState::Blank`] means only
//! that both A/B records are erased; it never authorizes ownership establishment, which needs
//! physical presence and a separate commissioning marker. An outer Retinue verifier may have
//! advanced before its counter was journaled: rebuild it from durable grants and counters
//! before accepting another envelope.
use super::*;
use crate::store::Slot;
use core::{convert::Infallible, fmt};
use heapless::Vec;
mod quiet;
use quiet::ActiveQuietGuard;
pub use quiet::{LiveOutcome, QuietExit, QuietGuard, QuietWindow};
#[cfg(feature = "control-retinue")]
mod inbound;
mod types;
mod verified;
pub use types::{
    BootState, ConfigApplier, DurableScratch, DurableScratchError, MAX_PROVISIONAL_LIFETIME_MS,
    MIN_DURABLE_SLOT_BYTES, MIN_PROVISIONAL_LIFETIME_MS, PreparedCommit, PreparedProvisional,
    RuntimeError,
};
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
