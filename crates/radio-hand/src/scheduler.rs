//! Bounded, radio-free lease admission for the resident listener.
//!
//! The scheduler accepts only caller-authenticated static assignments and peer
//! coverage promises. It recommends a lease or a return to listening; the
//! radio owner remains responsible for safe hardware boundaries and must
//! acknowledge that listening was actually restored.

use crate::profiles::ReceiveProfileId;

/// Fixed bounds keep the decision core allocation-free and make oversized
/// assignments a visible configuration error.
pub const MAX_REQUIRED_PROFILES: usize = 16;
pub const MAX_PERMITTED_COVERS: usize = 16;

/// Stable local identity for the peer designated to keep one profile covered.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PeerId(pub u8);

/// One caller-authenticated, flock-wide keeper assignment for a required
/// receive profile. Every board in the flock must use the same mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeeperAssignment {
    pub profile: ReceiveProfileId,
    pub keeper: PeerId,
}

/// The bounded work an adapter declares before it may borrow the radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseRequest {
    /// Airtime and all non-response work the adapter intends to perform.
    pub speaking_ms: u64,
    /// The worst-case acknowledgement or response window after that work.
    pub response_ms: u64,
}

/// A lease the caller may give to an adapter. Its deadline never moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseGrant {
    pub deadline_ms: u64,
    /// The latest time listening must be hardware-confirmed again.
    pub restore_by_ms: u64,
}

/// Why the radio owner must return to its listening plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreCause {
    Completed,
    LeaseDeadline,
    PeerCoverLost(ReceiveProfileId),
}

/// A return-to-listening obligation. This is a request, not evidence that the
/// hardware has already entered RX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestoreRequest {
    pub cause: RestoreCause,
    pub restore_by_ms: u64,
}

/// The only decisions this model emits. `RestoreRequired` remains in effect
/// until the radio owner calls [`Scheduler::acknowledge_listening`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerAction {
    KeepListening,
    LeaseGranted(LeaseGrant),
    /// An existing lease remains active. This is never a new TX authorization.
    LeaseActive(LeaseGrant),
    RestoreRequired(RestoreRequest),
    RestoreOverdue(RestoreRequest),
}

/// Invalid static scheduler policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    EmptyRequired,
    RequiredCapacity { supplied: usize, maximum: usize },
    CoverCapacity { supplied: usize, maximum: usize },
    DuplicateRequired(ReceiveProfileId),
    UnknownKeeperProfile(ReceiveProfileId),
    MissingKeeperAssignment(ReceiveProfileId),
    DuplicateKeeperAssignment(ReceiveProfileId),
    ZeroMaximumLease,
    ZeroRestoreBudget,
}

/// A caller-time failure. The caller supplies one monotonic millisecond clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeError {
    Regressed { previous_ms: u64, received_ms: u64 },
    Overflow,
}

/// A rejected peer coverage advertisement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverError {
    Time(TimeError),
    UnknownProfile(ReceiveProfileId),
    LocalKeeper(ReceiveProfileId),
    WrongKeeper {
        profile: ReceiveProfileId,
        expected: PeerId,
        received: PeerId,
    },
    Expired {
        profile: ReceiveProfileId,
        expires_at_ms: u64,
    },
    RecoveryAfterExpiry {
        profile: ReceiveProfileId,
    },
}

/// Why a requested speaking lease was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseRefusal {
    Time(TimeError),
    AlreadySpeaking,
    RestorePending,
    ZeroWork,
    ExceedsMaximum { requested_ms: u64, maximum_ms: u64 },
    MissingCover(ReceiveProfileId),
    LocallyDesignatedKeeper(ReceiveProfileId),
    CoverInRecovery(ReceiveProfileId),
    CoverExpiresBeforeRestore(ReceiveProfileId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LiveCover {
    expires_at_ms: u64,
    usable_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Listening,
    Speaking(LeaseGrant),
    Restoring(RestoreRequest),
}

/// Allocation-free decision core for one board's required listening floor.
///
/// `required` and `keepers` are locally admitted static policy. A transport
/// integration must authenticate the same keeper map on every flock member,
/// as well as every peer advertisement, before it
/// calls this type. A `PeerId` here is policy identity, not proof a radio peer
/// is present. The owner must also collect a completed RX and finish an
/// in-flight TX at a safe boundary before acting on `RestoreRequired`.
/// Keeper custody never rotates in this slice: a split map may deliberately
/// refuse every board's speaking lease, while a future rotation needs an
/// authenticated replacement assignment rather than a peer advertisement.
pub struct Scheduler<'a> {
    local_peer: PeerId,
    required: &'a [ReceiveProfileId],
    keepers: &'a [KeeperAssignment],
    covers: [Option<LiveCover>; MAX_PERMITTED_COVERS],
    maximum_lease_ms: u64,
    restore_budget_ms: u64,
    last_now_ms: Option<u64>,
    state: State,
}

impl<'a> Scheduler<'a> {
    /// Builds a scheduler over caller-owned static policy.
    pub fn new(
        local_peer: PeerId,
        required: &'a [ReceiveProfileId],
        keepers: &'a [KeeperAssignment],
        maximum_lease_ms: u64,
        restore_budget_ms: u64,
    ) -> Result<Self, ConfigError> {
        if required.is_empty() {
            return Err(ConfigError::EmptyRequired);
        }
        if required.len() > MAX_REQUIRED_PROFILES {
            return Err(ConfigError::RequiredCapacity {
                supplied: required.len(),
                maximum: MAX_REQUIRED_PROFILES,
            });
        }
        if keepers.len() > MAX_PERMITTED_COVERS {
            return Err(ConfigError::CoverCapacity {
                supplied: keepers.len(),
                maximum: MAX_PERMITTED_COVERS,
            });
        }
        if maximum_lease_ms == 0 {
            return Err(ConfigError::ZeroMaximumLease);
        }
        if restore_budget_ms == 0 {
            return Err(ConfigError::ZeroRestoreBudget);
        }
        for (index, profile) in required.iter().enumerate() {
            if required[..index].contains(profile) {
                return Err(ConfigError::DuplicateRequired(*profile));
            }
        }
        for assignment in keepers {
            if !required.contains(&assignment.profile) {
                return Err(ConfigError::UnknownKeeperProfile(assignment.profile));
            }
        }
        for profile in required {
            if !keepers
                .iter()
                .any(|assignment| assignment.profile == *profile)
            {
                return Err(ConfigError::MissingKeeperAssignment(*profile));
            }
        }
        for (index, assignment) in keepers.iter().enumerate() {
            if keepers[..index]
                .iter()
                .any(|known| known.profile == assignment.profile)
            {
                return Err(ConfigError::DuplicateKeeperAssignment(assignment.profile));
            }
        }

        Ok(Self {
            local_peer,
            required,
            keepers,
            covers: [None; MAX_PERMITTED_COVERS],
            maximum_lease_ms,
            restore_budget_ms,
            last_now_ms: None,
            state: State::Listening,
        })
    }

    /// Records an authenticated peer promise. `usable_after_ms` provides a
    /// caller-selected recovery hold; it can delay a recovered peer's use but
    /// can never extend its expiry.
    pub fn observe_cover(
        &mut self,
        now_ms: u64,
        peer: PeerId,
        profile: ReceiveProfileId,
        expires_at_ms: u64,
        usable_after_ms: u64,
    ) -> Result<(), CoverError> {
        self.accept_now(now_ms).map_err(CoverError::Time)?;
        self.latch_deadline_if_due(now_ms);
        let Some(index) = self.keeper_index(profile) else {
            return Err(CoverError::UnknownProfile(profile));
        };
        let expected = self.keepers[index].keeper;
        if expected == self.local_peer {
            return Err(CoverError::LocalKeeper(profile));
        }
        if peer != expected {
            return Err(CoverError::WrongKeeper {
                profile,
                expected,
                received: peer,
            });
        }
        if expires_at_ms <= now_ms {
            return Err(CoverError::Expired {
                profile,
                expires_at_ms,
            });
        }
        if usable_after_ms > expires_at_ms {
            return Err(CoverError::RecoveryAfterExpiry { profile });
        }
        self.covers[index] = Some(LiveCover {
            expires_at_ms,
            usable_at_ms: usable_after_ms,
        });
        Ok(())
    }

    /// Records an authenticated withdrawal or detected disappearance of the
    /// designated keeper. It does not grant a replacement or add hysteresis;
    /// a later advertisement may carry a recovery hold before re-admission.
    pub fn withdraw_cover(
        &mut self,
        now_ms: u64,
        peer: PeerId,
        profile: ReceiveProfileId,
    ) -> Result<(), CoverError> {
        self.accept_now(now_ms).map_err(CoverError::Time)?;
        self.latch_deadline_if_due(now_ms);
        let Some(index) = self.keeper_index(profile) else {
            return Err(CoverError::UnknownProfile(profile));
        };
        let expected = self.keepers[index].keeper;
        if expected == self.local_peer {
            return Err(CoverError::LocalKeeper(profile));
        }
        if peer != expected {
            return Err(CoverError::WrongKeeper {
                profile,
                expected,
                received: peer,
            });
        }
        self.covers[index] = None;
        if let State::Speaking(grant) = self.state {
            self.begin_restore(
                now_ms,
                RestoreCause::PeerCoverLost(profile),
                grant.restore_by_ms,
            );
        }
        Ok(())
    }

    /// Advances time and reports an outstanding restore obligation. Calling it
    /// regularly is how the owner notices a deadline or peer expiry.
    pub fn poll(&mut self, now_ms: u64) -> Result<SchedulerAction, TimeError> {
        self.accept_now(now_ms)?;
        match self.state {
            State::Listening => Ok(SchedulerAction::KeepListening),
            State::Speaking(grant) => Ok(self.advance_speaking(now_ms, grant)),
            State::Restoring(request) if now_ms > request.restore_by_ms => {
                Ok(SchedulerAction::RestoreOverdue(request))
            }
            State::Restoring(request) if now_ms > request.restore_by_ms => {
                Ok(SchedulerAction::RestoreOverdue(request))
            }
            State::Restoring(request) => Ok(SchedulerAction::RestoreRequired(request)),
        }
    }

    /// Admits one bounded speaking lease only if every locally required profile
    /// has a usable, designated peer cover through the restore deadline.
    pub fn request_lease(
        &mut self,
        now_ms: u64,
        request: LeaseRequest,
    ) -> Result<SchedulerAction, LeaseRefusal> {
        self.accept_now(now_ms).map_err(LeaseRefusal::Time)?;
        match self.state {
            State::Speaking(_) => return Err(LeaseRefusal::AlreadySpeaking),
            State::Restoring(_) => return Err(LeaseRefusal::RestorePending),
            State::Listening => {}
        }
        let Some(work_ms) = request.speaking_ms.checked_add(request.response_ms) else {
            return Err(LeaseRefusal::ExceedsMaximum {
                requested_ms: u64::MAX,
                maximum_ms: self.maximum_lease_ms,
            });
        };
        if work_ms == 0 {
            return Err(LeaseRefusal::ZeroWork);
        }
        if work_ms > self.maximum_lease_ms {
            return Err(LeaseRefusal::ExceedsMaximum {
                requested_ms: work_ms,
                maximum_ms: self.maximum_lease_ms,
            });
        }
        let Some(deadline_ms) = now_ms.checked_add(work_ms) else {
            return Err(LeaseRefusal::Time(TimeError::Overflow));
        };
        let Some(restore_by_ms) = deadline_ms.checked_add(self.restore_budget_ms) else {
            return Err(LeaseRefusal::Time(TimeError::Overflow));
        };
        for profile in self.required {
            let index = self
                .keeper_index(*profile)
                .expect("validated at construction");
            if self.keepers[index].keeper == self.local_peer {
                return Err(LeaseRefusal::LocallyDesignatedKeeper(*profile));
            }
            let Some(cover) = self.covers[index] else {
                return Err(LeaseRefusal::MissingCover(*profile));
            };
            if now_ms < cover.usable_at_ms {
                return Err(LeaseRefusal::CoverInRecovery(*profile));
            }
            if cover.expires_at_ms < restore_by_ms {
                return Err(LeaseRefusal::CoverExpiresBeforeRestore(*profile));
            }
        }
        let grant = LeaseGrant {
            deadline_ms,
            restore_by_ms,
        };
        self.state = State::Speaking(grant);
        Ok(SchedulerAction::LeaseGranted(grant))
    }

    /// Ends an ordinary adapter exchange. The owner must still restore hardware
    /// and call [`Scheduler::acknowledge_listening`].
    pub fn finish_lease(&mut self, now_ms: u64) -> Result<SchedulerAction, TimeError> {
        self.accept_now(now_ms)?;
        match self.state {
            State::Speaking(grant) if now_ms >= grant.deadline_ms => {
                Ok(self.begin_restore(now_ms, RestoreCause::LeaseDeadline, grant.restore_by_ms))
            }
            State::Speaking(grant) => {
                Ok(self.begin_restore(now_ms, RestoreCause::Completed, grant.restore_by_ms))
            }
            State::Restoring(request) if now_ms > request.restore_by_ms => {
                Ok(SchedulerAction::RestoreOverdue(request))
            }
            State::Restoring(request) => Ok(SchedulerAction::RestoreRequired(request)),
            State::Listening => Ok(SchedulerAction::KeepListening),
        }
    }

    /// Hardware-confirmed return to the required listening plan.
    pub fn acknowledge_listening(&mut self, now_ms: u64) -> Result<SchedulerAction, TimeError> {
        self.accept_now(now_ms)?;
        match self.state {
            State::Restoring(request) if now_ms > request.restore_by_ms => {
                Ok(SchedulerAction::RestoreOverdue(request))
            }
            State::Restoring(_) => {
                self.state = State::Listening;
                Ok(SchedulerAction::KeepListening)
            }
            State::Listening => Ok(SchedulerAction::KeepListening),
            State::Speaking(grant) => Ok(self.advance_speaking(now_ms, grant)),
        }
    }

    fn accept_now(&mut self, now_ms: u64) -> Result<(), TimeError> {
        if let Some(previous_ms) = self.last_now_ms
            && now_ms < previous_ms
        {
            return Err(TimeError::Regressed {
                previous_ms,
                received_ms: now_ms,
            });
        }
        self.last_now_ms = Some(now_ms);
        Ok(())
    }

    fn keeper_index(&self, profile: ReceiveProfileId) -> Option<usize> {
        self.keepers
            .iter()
            .position(|cover| cover.profile == profile)
    }

    fn lost_required_cover(&self, now_ms: u64) -> Option<ReceiveProfileId> {
        self.required.iter().copied().find(|profile| {
            let Some(index) = self.keeper_index(*profile) else {
                return true;
            };
            let Some(cover) = self.covers[index] else {
                return true;
            };
            now_ms < cover.usable_at_ms || now_ms >= cover.expires_at_ms
        })
    }

    fn latch_deadline_if_due(&mut self, now_ms: u64) {
        if let State::Speaking(grant) = self.state
            && now_ms >= grant.deadline_ms
        {
            self.begin_restore(now_ms, RestoreCause::LeaseDeadline, grant.restore_by_ms);
        }
    }

    fn advance_speaking(&mut self, now_ms: u64, grant: LeaseGrant) -> SchedulerAction {
        if now_ms >= grant.deadline_ms {
            self.begin_restore(now_ms, RestoreCause::LeaseDeadline, grant.restore_by_ms)
        } else if let Some(profile) = self.lost_required_cover(now_ms) {
            self.begin_restore(
                now_ms,
                RestoreCause::PeerCoverLost(profile),
                grant.restore_by_ms,
            )
        } else {
            SchedulerAction::LeaseActive(grant)
        }
    }

    fn begin_restore(
        &mut self,
        now_ms: u64,
        cause: RestoreCause,
        lease_restore_by_ms: u64,
    ) -> SchedulerAction {
        let restore_by_ms = now_ms
            .checked_add(self.restore_budget_ms)
            .map_or(lease_restore_by_ms, |candidate| {
                candidate.min(lease_restore_by_ms)
            });
        let request = RestoreRequest {
            cause,
            restore_by_ms,
        };
        self.state = State::Restoring(request);
        if now_ms > request.restore_by_ms {
            SchedulerAction::RestoreOverdue(request)
        } else {
            SchedulerAction::RestoreRequired(request)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: ReceiveProfileId = ReceiveProfileId(0x12);
    const B: ReceiveProfileId = ReceiveProfileId(0x2b);
    const REQUIRED: [ReceiveProfileId; 2] = [A, B];
    const KEEPERS: [KeeperAssignment; 2] = [
        KeeperAssignment {
            profile: A,
            keeper: PeerId(1),
        },
        KeeperAssignment {
            profile: B,
            keeper: PeerId(2),
        },
    ];

    fn scheduler() -> Scheduler<'static> {
        Scheduler::new(PeerId(0), &REQUIRED, &KEEPERS, 100, 10).unwrap()
    }

    fn cover_all(scheduler: &mut Scheduler<'_>, now: u64, expiry: u64) {
        scheduler
            .observe_cover(now, PeerId(1), A, expiry, now)
            .unwrap();
        scheduler
            .observe_cover(now, PeerId(2), B, expiry, now)
            .unwrap();
    }

    #[test]
    fn ordinary_finish_requires_hardware_ack_before_another_lease() {
        let mut scheduler = scheduler();
        cover_all(&mut scheduler, 10, 100);
        assert_eq!(
            scheduler.request_lease(
                10,
                LeaseRequest {
                    speaking_ms: 20,
                    response_ms: 5
                }
            ),
            Ok(SchedulerAction::LeaseGranted(LeaseGrant {
                deadline_ms: 35,
                restore_by_ms: 45
            }))
        );
        assert_eq!(
            scheduler.finish_lease(25),
            Ok(SchedulerAction::RestoreRequired(RestoreRequest {
                cause: RestoreCause::Completed,
                restore_by_ms: 35
            }))
        );
        assert_eq!(
            scheduler.request_lease(
                25,
                LeaseRequest {
                    speaking_ms: 1,
                    response_ms: 0
                }
            ),
            Err(LeaseRefusal::RestorePending)
        );
        assert_eq!(
            scheduler.acknowledge_listening(30),
            Ok(SchedulerAction::KeepListening)
        );
    }

    #[test]
    fn exact_expiry_during_a_lease_requests_restore_and_never_reuses_cover() {
        let mut scheduler = scheduler();
        cover_all(&mut scheduler, 10, 100);
        scheduler
            .request_lease(
                10,
                LeaseRequest {
                    speaking_ms: 20,
                    response_ms: 5,
                },
            )
            .unwrap();
        scheduler.observe_cover(20, PeerId(1), A, 25, 20).unwrap();
        assert_eq!(
            scheduler.poll(24),
            Ok(SchedulerAction::LeaseActive(LeaseGrant {
                deadline_ms: 35,
                restore_by_ms: 45
            }))
        );
        assert_eq!(
            scheduler.poll(25),
            Ok(SchedulerAction::RestoreRequired(RestoreRequest {
                cause: RestoreCause::PeerCoverLost(A),
                restore_by_ms: 35
            }))
        );
        assert_eq!(
            scheduler.acknowledge_listening(25),
            Ok(SchedulerAction::KeepListening)
        );
        assert_eq!(
            scheduler.request_lease(
                25,
                LeaseRequest {
                    speaking_ms: 1,
                    response_ms: 0
                }
            ),
            Err(LeaseRefusal::CoverExpiresBeforeRestore(A))
        );
    }

    #[test]
    fn peer_loss_before_deadline_preempts_future_talk_at_safe_boundary() {
        let mut scheduler = scheduler();
        cover_all(&mut scheduler, 10, 80);
        scheduler
            .request_lease(
                10,
                LeaseRequest {
                    speaking_ms: 20,
                    response_ms: 5,
                },
            )
            .unwrap();
        scheduler.withdraw_cover(20, PeerId(1), A).unwrap();
        assert_eq!(
            scheduler.poll(20),
            Ok(SchedulerAction::RestoreRequired(RestoreRequest {
                cause: RestoreCause::PeerCoverLost(A),
                restore_by_ms: 30
            }))
        );
        assert_eq!(
            scheduler.request_lease(
                20,
                LeaseRequest {
                    speaking_ms: 1,
                    response_ms: 0
                }
            ),
            Err(LeaseRefusal::RestorePending)
        );
    }

    #[test]
    fn deadline_and_missing_restore_ack_are_visible() {
        let mut scheduler = scheduler();
        cover_all(&mut scheduler, 0, 100);
        scheduler
            .request_lease(
                0,
                LeaseRequest {
                    speaking_ms: 20,
                    response_ms: 5,
                },
            )
            .unwrap();
        assert_eq!(
            scheduler.poll(25),
            Ok(SchedulerAction::RestoreRequired(RestoreRequest {
                cause: RestoreCause::LeaseDeadline,
                restore_by_ms: 35
            }))
        );
        assert_eq!(
            scheduler.poll(36),
            Ok(SchedulerAction::RestoreOverdue(RestoreRequest {
                cause: RestoreCause::LeaseDeadline,
                restore_by_ms: 35
            }))
        );
        assert_eq!(
            scheduler.acknowledge_listening(36),
            Ok(SchedulerAction::RestoreOverdue(RestoreRequest {
                cause: RestoreCause::LeaseDeadline,
                restore_by_ms: 35
            }))
        );
        assert_eq!(
            scheduler.finish_lease(36),
            Ok(SchedulerAction::RestoreOverdue(RestoreRequest {
                cause: RestoreCause::LeaseDeadline,
                restore_by_ms: 35
            }))
        );
    }

    #[test]
    fn withdrawal_latches_restore_even_if_the_peer_renews_before_poll() {
        let mut scheduler = scheduler();
        cover_all(&mut scheduler, 10, 100);
        scheduler
            .request_lease(
                10,
                LeaseRequest {
                    speaking_ms: 20,
                    response_ms: 5,
                },
            )
            .unwrap();
        scheduler.withdraw_cover(20, PeerId(1), A).unwrap();
        scheduler.observe_cover(20, PeerId(1), A, 100, 20).unwrap();
        assert_eq!(
            scheduler.poll(20),
            Ok(SchedulerAction::RestoreRequired(RestoreRequest {
                cause: RestoreCause::PeerCoverLost(A),
                restore_by_ms: 30
            }))
        );
    }

    #[test]
    fn acknowledgement_at_a_deadline_cannot_represent_a_live_lease() {
        let mut scheduler = scheduler();
        cover_all(&mut scheduler, 0, 100);
        scheduler
            .request_lease(
                0,
                LeaseRequest {
                    speaking_ms: 20,
                    response_ms: 5,
                },
            )
            .unwrap();
        assert_eq!(
            scheduler.acknowledge_listening(25),
            Ok(SchedulerAction::RestoreRequired(RestoreRequest {
                cause: RestoreCause::LeaseDeadline,
                restore_by_ms: 35
            }))
        );
    }

    #[test]
    fn recovery_hold_delays_re_admission_without_extending_expiry() {
        let mut scheduler = scheduler();
        scheduler.observe_cover(10, PeerId(1), A, 100, 30).unwrap();
        scheduler.observe_cover(10, PeerId(2), B, 100, 10).unwrap();
        assert_eq!(
            scheduler.request_lease(
                20,
                LeaseRequest {
                    speaking_ms: 10,
                    response_ms: 0
                }
            ),
            Err(LeaseRefusal::CoverInRecovery(A))
        );
        assert!(matches!(
            scheduler.request_lease(
                30,
                LeaseRequest {
                    speaking_ms: 10,
                    response_ms: 0
                }
            ),
            Ok(SchedulerAction::LeaseGranted(_))
        ));
    }

    #[test]
    fn time_overflow_and_regression_are_refused() {
        let mut scheduler = scheduler();
        cover_all(&mut scheduler, 10, u64::MAX);
        assert_eq!(
            scheduler.poll(9),
            Err(TimeError::Regressed {
                previous_ms: 10,
                received_ms: 9
            })
        );
        assert_eq!(
            scheduler.request_lease(
                u64::MAX,
                LeaseRequest {
                    speaking_ms: 1,
                    response_ms: 0
                }
            ),
            Err(LeaseRefusal::Time(TimeError::Overflow))
        );
    }

    #[test]
    fn invalid_capacity_and_unknown_or_unauthorized_profiles_are_explicit() {
        let too_many = [A; MAX_REQUIRED_PROFILES + 1];
        assert_eq!(
            Scheduler::new(PeerId(0), &too_many, &[], 1, 1).err(),
            Some(ConfigError::RequiredCapacity {
                supplied: MAX_REQUIRED_PROFILES + 1,
                maximum: MAX_REQUIRED_PROFILES
            })
        );
        assert_eq!(
            Scheduler::new(
                PeerId(0),
                &[A],
                &[KeeperAssignment {
                    profile: B,
                    keeper: PeerId(1)
                }],
                1,
                1
            )
            .err(),
            Some(ConfigError::UnknownKeeperProfile(B))
        );
        let mut scheduler = scheduler();
        assert_eq!(
            scheduler.observe_cover(1, PeerId(9), A, 10, 1),
            Err(CoverError::WrongKeeper {
                profile: A,
                expected: PeerId(1),
                received: PeerId(9)
            })
        );
        assert_eq!(
            scheduler.observe_cover(1, PeerId(1), ReceiveProfileId(7), 10, 1),
            Err(CoverError::UnknownProfile(ReceiveProfileId(7)))
        );
    }

    #[test]
    fn designated_keeper_cannot_delegate_to_make_two_peers_each_sole_keeper() {
        const PROFILE: ReceiveProfileId = ReceiveProfileId(4);
        const ONE: [ReceiveProfileId; 1] = [PROFILE];
        const MAP: [KeeperAssignment; 1] = [KeeperAssignment {
            profile: PROFILE,
            keeper: PeerId(1),
        }];

        let mut delegated = Scheduler::new(PeerId(2), &ONE, &MAP, 10, 2).unwrap();
        delegated
            .observe_cover(0, PeerId(1), PROFILE, 20, 0)
            .unwrap();
        assert!(matches!(
            delegated.request_lease(
                0,
                LeaseRequest {
                    speaking_ms: 1,
                    response_ms: 0
                }
            ),
            Ok(SchedulerAction::LeaseGranted(_))
        ));

        let mut keeper = Scheduler::new(PeerId(1), &ONE, &MAP, 10, 2).unwrap();
        assert_eq!(
            keeper.request_lease(
                0,
                LeaseRequest {
                    speaking_ms: 1,
                    response_ms: 0
                }
            ),
            Err(LeaseRefusal::LocallyDesignatedKeeper(PROFILE))
        );
        assert_eq!(
            keeper.observe_cover(0, PeerId(2), PROFILE, 20, 0),
            Err(CoverError::LocalKeeper(PROFILE))
        );
    }
}
