//! Durable scratch, applier seam, prepared transitions, and runtime errors.

use super::*;

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
    pub(super) slot_a: &'a mut [u8],
    pub(super) slot_b: &'a mut [u8],
    pub(super) body: &'a mut [u8; MAX_DURABLE_BODY],
    pub(super) page: &'a mut [u8],
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
