use super::*;
use crate::identity::LocalIdentity;

fn instance(seed: u8) -> Instance {
    Instance::new(
        Node::new(LocalIdentity::from_seed([seed; 32]), false),
        InstanceConfig::new(2, 10).unwrap(),
    )
}

fn introduce(a: &mut Instance, b: &mut Instance) {
    let a_adv = a.active_node_mut().unwrap().advert_frame(1, b"a");
    let b_adv = b.active_node_mut().unwrap().advert_frame(2, b"b");
    b.on_frame(0, &a_adv).unwrap();
    a.on_frame(0, &b_adv).unwrap();
}

#[test]
fn two_nodes_ack_a_retained_send() {
    let mut a = instance(1);
    let mut b = instance(2);
    introduce(&mut a, &mut b);
    let id = a
        .begin_send(
            1,
            b.node().my_hash(),
            "hello",
            TextRetryPolicy::default(),
            SendTiming {
                timestamp: 3,
                expires_at: 100,
                allowed_until: 100,
            },
        )
        .unwrap();
    let attempt = a.next_retry(1, id, 2).unwrap().unwrap();
    let received = b.on_frame(1, &attempt.frame).unwrap();
    let ack = match received.events.as_slice() {
        [Event::Message { ack, .. }] => *ack,
        _ => panic!("text"),
    };
    let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
    assert_eq!(a.on_frame(2, &ack_frame).unwrap().acknowledged, [id]);
    assert_eq!(a.pending_len(), 0);
}

#[test]
fn delayed_ack_from_prior_attempt_matches() {
    let mut a = instance(3);
    let mut b = instance(4);
    introduce(&mut a, &mut b);
    let id = a
        .begin_send(
            1,
            b.node().my_hash(),
            "retry",
            TextRetryPolicy::default(),
            SendTiming {
                timestamp: 3,
                expires_at: 100,
                allowed_until: 100,
            },
        )
        .unwrap();
    let first = a.next_retry(1, id, 2).unwrap().unwrap();
    let _second = a.next_retry(11, id, 12).unwrap().unwrap();
    let received = b.on_frame(12, &first.frame).unwrap();
    let ack = match received.events.as_slice() {
        [Event::Message { ack, .. }] => *ack,
        _ => panic!("text"),
    };
    let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
    assert_eq!(a.on_frame(13, &ack_frame).unwrap().acknowledged, [id]);
}

#[test]
fn final_attempt_waits_for_ack_but_expiry_wins_at_its_exact_boundary() {
    let mut a = instance(12);
    let mut b = instance(13);
    introduce(&mut a, &mut b);
    let one_try = TextRetryPolicy::new(1, false).unwrap();
    let id = a
        .begin_send(
            1,
            b.node().my_hash(),
            "final",
            one_try,
            SendTiming {
                timestamp: 3,
                expires_at: 50,
                allowed_until: 49,
            },
        )
        .unwrap();
    let sent = a.next_retry(1, id, 2).unwrap().unwrap();
    let received = b.on_frame(2, &sent.frame).unwrap();
    let ack = match received.events.as_slice() {
        [Event::Message { ack, .. }] => *ack,
        _ => panic!("text"),
    };
    assert!(a.next_retry(11, id, 12).unwrap().is_none());
    assert_eq!(a.operations().collect::<Vec<_>>()[0].id, id);
    assert_eq!(a.next_deadline(), Some(50));
    assert_eq!(a.assess_pause(12, 49), Ok(PauseAssessment::Ready));
    let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
    assert_eq!(a.on_frame(12, &ack_frame).unwrap().acknowledged, [id]);

    let expired = a
        .begin_send(
            20,
            b.node().my_hash(),
            "late",
            one_try,
            SendTiming {
                timestamp: 4,
                expires_at: 30,
                allowed_until: 29,
            },
        )
        .unwrap();
    let sent = a.next_retry(20, expired, 21).unwrap().unwrap();
    let received = b.on_frame(21, &sent.frame).unwrap();
    let ack = match received.events.as_slice() {
        [Event::Message { ack, .. }] => *ack,
        _ => panic!("text"),
    };
    let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
    let outcome = a.on_frame(30, &ack_frame).unwrap();
    assert_eq!(outcome.expired.operations, [expired]);
    assert!(outcome.acknowledged.is_empty());
}

#[test]
fn pause_requires_loss_then_resume_expires_without_sending() {
    let mut a = instance(5);
    let mut b = instance(6);
    introduce(&mut a, &mut b);
    let id = a
        .begin_send(
            1,
            b.node().my_hash(),
            "hold",
            TextRetryPolicy::default(),
            SendTiming {
                timestamp: 3,
                expires_at: 20,
                allowed_until: 20,
            },
        )
        .unwrap();
    assert!(
        matches!(a.assess_pause(2, 21), Ok(PauseAssessment::RequiresLoss { pending }) if pending == [id])
    );
    assert_eq!(a.pause(2, 21), Err(InstanceError::ReturnBudgetExceeded));
    let _ = a.next_retry(1, id, 2).unwrap().unwrap();
    a.pause(2, 3).unwrap();
    assert!(matches!(a.on_frame(3, b"bad"), Err(InstanceError::Paused)));
    assert_eq!(a.resume(21).unwrap().operations, [id]);
}

#[test]
fn denial_is_pure_and_time_regression_refuses() {
    let mut a = instance(7);
    let mut b = instance(8);
    introduce(&mut a, &mut b);
    let id = a
        .begin_send(
            1,
            b.node().my_hash(),
            "hold",
            TextRetryPolicy::default(),
            SendTiming {
                timestamp: 3,
                expires_at: 100,
                allowed_until: 100,
            },
        )
        .unwrap();
    assert_eq!(
        a.interrupt(LossPermission::Deny),
        Err(InstanceError::LossNotPermitted)
    );
    assert_eq!(a.pending_len(), 1);
    assert_eq!(a.next_retry(0, id, 1), Err(InstanceError::TimeRegression));
    assert_eq!(a.interrupt(LossPermission::Allow).unwrap().operations, [id]);
}

#[test]
fn pending_capacity_invalid_return_and_retry_overflow_refuse_before_mutation() {
    let mut a = Instance::new(
        Node::new(LocalIdentity::from_seed([9; 32]), false),
        InstanceConfig::new(1, 10).unwrap(),
    );
    let mut b = instance(10);
    introduce(&mut a, &mut b);
    let id = a
        .begin_send(
            1,
            b.node().my_hash(),
            "one",
            TextRetryPolicy::default(),
            SendTiming {
                timestamp: 3,
                expires_at: 100,
                allowed_until: 100,
            },
        )
        .unwrap();
    assert_eq!(a.assess_pause(2, 1), Err(InstanceError::InvalidDeadline));
    assert_eq!(
        a.begin_send(
            2,
            b.node().my_hash(),
            "two",
            TextRetryPolicy::default(),
            SendTiming {
                timestamp: 3,
                expires_at: 100,
                allowed_until: 100
            },
        ),
        Err(InstanceError::PendingFull)
    );
    assert_eq!(a.pending_len(), 1);

    let mut overflow = Instance::new(
        Node::new(LocalIdentity::from_seed([11; 32]), false),
        InstanceConfig::new(1, 2).unwrap(),
    );
    overflow
        .active_node_mut()
        .unwrap()
        .try_add_contact(b.node().identity().clone())
        .unwrap();
    let overflow_id = overflow
        .begin_send(
            u64::MAX - 1,
            b.node().my_hash(),
            "x",
            TextRetryPolicy::default(),
            SendTiming {
                timestamp: 3,
                expires_at: u64::MAX,
                allowed_until: u64::MAX - 1,
            },
        )
        .unwrap();
    assert_eq!(
        overflow.next_retry(u64::MAX - 1, overflow_id, u64::MAX - 1),
        Err(InstanceError::TimeOverflow)
    );
    assert_eq!(overflow.pending_len(), 1);
    assert_eq!(id, OperationId(1));
}
