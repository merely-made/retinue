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
