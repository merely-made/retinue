//! Configuration, requests, states and errors of the excursion controller.

pub const MAX_INSTALLED_PERSONALITIES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PersonalityId(pub u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstalledPersonalitySet {
    entries: [Option<PersonalityId>; MAX_INSTALLED_PERSONALITIES],
    len: usize,
}

impl InstalledPersonalitySet {
    pub fn new(entries: &[PersonalityId]) -> Result<Self, ConfigError> {
        if entries.len() > MAX_INSTALLED_PERSONALITIES {
            return Err(ConfigError::TooManyInstalled);
        }
        let mut result = Self {
            entries: [None; MAX_INSTALLED_PERSONALITIES],
            len: 0,
        };
        for &entry in entries {
            if result.contains(entry) {
                return Err(ConfigError::DuplicateInstalled(entry));
            }
            result.entries[result.len] = Some(entry);
            result.len += 1;
        }
        Ok(result)
    }
    pub fn contains(&self, personality: PersonalityId) -> bool {
        self.entries[..self.len].contains(&Some(personality))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptionPolicy {
    ResumableOnly,
    AllowSessionLoss,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopCapability {
    Resumable,
    RequiresSessionLoss,
    Unsupported,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseOutcome {
    Ready,
    Busy { retry_at: u64 },
    RequiresSessionLoss { affected_sessions: u16 },
    Unsupported,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoveragePolicy {
    AllowGap,
    RequireCoverage,
}

/// Finite caller-supplied evidence. It is neither a radio receipt nor proof of peer identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageEvidence {
    pub valid_until: Option<u64>,
}
impl CoverageEvidence {
    pub fn is_valid_at(self, now: u64) -> bool {
        self.valid_until.is_some_and(|until| until >= now)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerConfig {
    pub home: PersonalityId,
    pub pin: Option<PersonalityId>,
    pub installed: InstalledPersonalitySet,
    pub coverage: CoveragePolicy,
    pub max_excursion_ms: u64,
    pub return_budget_ms: u64,
    pub max_defer_ms: u64,
    pub transition_timeout_ms: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Excursion {
    pub target: PersonalityId,
    pub duration_ms: u64,
    pub interruption: InterruptionPolicy,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition {
    pub id: u64,
    pub from: PersonalityId,
    pub to: PersonalityId,
    pub deadline: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Acknowledgement {
    Completed,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnReason {
    /// The excursion's requested work completed successfully.
    Completed,
    Cancelled,
    ExcursionExpired,
    CoverageLost,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerState {
    Home,
    DeferredExcursion {
        request: Excursion,
        coverage: CoverageEvidence,
        retry_at: u64,
        defer_deadline: u64,
    },
    Transitioning {
        transition: Transition,
        request: Option<Excursion>,
        pending_return: Option<ReturnReason>,
    },
    Away {
        target: PersonalityId,
        excursion_deadline: u64,
        return_by: u64,
        interruption: InterruptionPolicy,
    },
    ReturnRequired {
        target: PersonalityId,
        reason: ReturnReason,
        return_by: u64,
        interruption: InterruptionPolicy,
    },
    DeferredReturn {
        target: PersonalityId,
        reason: ReturnReason,
        retry_at: u64,
        return_by: u64,
        interruption: InterruptionPolicy,
    },
    RecoveryRequired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerEvent {
    Idle,
    ReturnRequired(ReturnReason),
    RecoveryRequired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    TooManyInstalled,
    DuplicateInstalled(PersonalityId),
    HomeNotInstalled,
    PinNotInstalled(PersonalityId),
    PinDiffersFromHome {
        pin: PersonalityId,
        home: PersonalityId,
    },
    ZeroDuration,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerError {
    ClockRegression { previous: u64, received: u64 },
    TimeOverflow,
    NotHome,
    NotReturning,
    RecoveryRequired,
    Unsupported(PersonalityId),
    Pinned(PersonalityId),
    HomeIsNotAnExcursion,
    ExcursionTooLong,
    CoverageUnavailable,
    TargetCannotReturn,
    SessionLossNotAllowed { affected_sessions: u16 },
    PauseUnsupported,
    RetryBeforeNow { retry_at: u64, now: u64 },
    DeferDeadlineExceeded,
    WrongAcknowledgement { expected: u64, received: u64 },
}
