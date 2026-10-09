use super::*;
use crate::{
    flood::{ManagedFloodConfig, RelayDelayWindow},
    node::{Channel, NodeError},
    node_info::NodeDirectoryConfig,
    transport::{BROADCAST_DESTINATION, ChannelKey, Header},
};

fn config() -> SennetInstanceConfig {
    SennetInstanceConfig {
        channel: Channel {
            hash: 8,
            key: ChannelKey::Aes128([7; 16]),
        },
        flood: ManagedFloodConfig {
            channel_hash: 8,
            relay_node: 4,
            seen_capacity: 2,
            delay: RelayDelayWindow::new(core::time::Duration::ZERO, core::time::Duration::ZERO)
                .unwrap(),
        },
        directory: NodeDirectoryConfig::default(),
        pending_ttl: 10,
    }
}
fn instance() -> SennetInstance {
    SennetInstance::new(
        config(),
        PacketIdLease::new(0x0102_0304, 10, 13, 10).unwrap(),
    )
    .unwrap()
}
fn header() -> Header {
    Header {
        destination: BROADCAST_DESTINATION,
        source: 0,
        packet_id: 0,
        hop_limit: 3,
        want_ack: false,
        via_mqtt: false,
        hop_start: 3,
        channel_hash: 0,
        next_hop: 0,
        relay_node: 4,
    }
}

#[test]
fn retained_channel_ids_and_dedup_survive_lifecycle() {
    let mut instance = instance();
    let first = instance.queue_text(1, header(), "one").unwrap();
    assert_eq!(first.packet_id, 10);
    let outbound = instance.take_outbound(1).unwrap().unwrap();
    assert_eq!(
        instance
            .complete_tx(2, outbound.operation_id, outbound.identity)
            .unwrap(),
        InstanceEvent::TxCompleted {
            operation_id: outbound.operation_id,
            identity: first
        }
    );
    let second = instance.queue_text(3, header(), "two").unwrap();
    assert_eq!(second.packet_id, 11);
    let frame = instance.take_outbound(3).unwrap().unwrap().frame;
    assert!(matches!(
        instance.receive(4, &frame).unwrap(),
        ReceiveOutcome::Text(_)
    ));
    assert_eq!(
        instance.receive(5, &frame).unwrap(),
        ReceiveOutcome::Duplicate { identity: second }
    );
    assert_eq!(instance.seen_len(), 1);
}

#[test]
fn denied_pause_does_not_mutate_state() {
    let mut instance = instance();
    let identity = instance.queue_text(10, header(), "wait").unwrap();
    assert_eq!(
        instance.assess_pause(11, 15),
        Ok(PauseOutcome::Busy { retry_at: 20 })
    );
    assert_eq!(
        instance.pause(11, 15),
        Err(InstanceError::PauseNotReady(PauseOutcome::Busy {
            retry_at: 20
        }))
    );
    assert!(!instance.is_paused());
    assert_eq!(instance.pending_identity(), Some(identity));
    assert_eq!(instance.packet_id_state().next_packet_id(), 11);
}

#[test]
fn absence_advances_time_and_requires_explicit_loss() {
    let mut instance = instance();
    let identity = instance.queue_text(1, header(), "loss").unwrap();
    assert_eq!(
        instance.assess_pause(2, 11),
        Ok(PauseOutcome::RequiresLoss { pending: 1 })
    );
    assert_eq!(
        instance
            .discard_pending(2, LossPermission::operator())
            .unwrap(),
        Some(InstanceEvent::PendingLost {
            operation_id: 1,
            identity
        })
    );
    instance.pause(2, 11).unwrap();
    assert!(instance.resume(12).unwrap().is_none());
    assert_eq!(instance.packet_id_state().next_packet_id(), 11);
}

#[test]
fn expiry_time_and_lease_extension_are_checked() {
    let mut instance = instance();
    let identity = instance.queue_text(1, header(), "expiry").unwrap();
    assert_eq!(
        instance.advance(11).unwrap(),
        Some(InstanceEvent::PendingExpired {
            operation_id: 1,
            identity
        })
    );
    assert_eq!(
        instance.advance(10),
        Err(InstanceError::TimeRegressed {
            previous: 11,
            now: 10
        })
    );
    assert_eq!(
        instance.extend_lease(
            PacketIdReservation::new(0x0102_0304, 13, 15).unwrap(),
            ReservationProof::durable_ack()
        ),
        Ok(())
    );
    assert_eq!(
        instance.extend_lease(
            PacketIdReservation::new(0x0102_0304, 14, 16).unwrap(),
            ReservationProof::trusted_caller()
        ),
        Err(InstanceError::LeaseNotContiguous {
            expected_start: 15,
            actual_start: 14
        })
    );
    let overflow = SennetInstance::new(
        SennetInstanceConfig {
            pending_ttl: u64::MAX,
            ..config()
        },
        PacketIdLease::new(1, 1, 2, 1).unwrap(),
    )
    .unwrap();
    let mut overflow = overflow;
    assert_eq!(
        overflow.queue_text(1, header(), "x"),
        Err(InstanceError::DeadlineOverflow {
            now: 1,
            ttl: u64::MAX
        })
    );
}

#[test]
fn paused_operations_are_refused() {
    let mut instance = instance();
    instance.pause(1, 1).unwrap();
    assert_eq!(
        instance.queue_text(1, header(), "no"),
        Err(InstanceError::Paused)
    );
    assert_eq!(instance.take_outbound(1), Err(InstanceError::Paused));
    assert_eq!(instance.receive(1, &[]), Err(InstanceError::Paused));
}

#[test]
fn invalid_composition_does_not_burn_ids_or_operations() {
    let mut instance = instance();
    let mut invalid = header();
    invalid.hop_limit = 8;
    assert!(matches!(
        instance.queue_text(1, invalid, "bad"),
        Err(InstanceError::Node(NodeError::Transport(_)))
    ));
    let too_long = "x".repeat(crate::application::MAX_APPLICATION_PAYLOAD + 1);
    assert!(matches!(
        instance.queue_text(1, header(), &too_long),
        Err(InstanceError::Node(NodeError::Application(_)))
    ));
    assert_eq!(instance.packet_id_state().next_packet_id(), 10);
    assert_eq!(instance.pending_operation_id(), None);
    let identity = instance.queue_text(1, header(), "good").unwrap();
    let outbound = instance.take_outbound(1).unwrap().unwrap();
    assert_eq!(identity.packet_id, 10);
    assert_eq!(outbound.operation_id, 1);
}

#[test]
fn expiry_boundary_stays_queued_until_advance_accounts_for_it() {
    let mut instance = instance();
    let identity = instance.queue_text(1, header(), "deadline").unwrap();
    assert_eq!(instance.next_deadline(), Some(11));
    assert_eq!(
        instance.take_outbound(11),
        Err(InstanceError::PendingExpired {
            operation_id: 1,
            identity
        })
    );
    assert_eq!(
        instance.advance(11).unwrap(),
        Some(InstanceEvent::PendingExpired {
            operation_id: 1,
            identity
        })
    );
    assert_eq!(instance.pending_identity(), None);
}

#[test]
fn queued_completion_is_refused_without_mutation() {
    let mut instance = instance();
    let identity = instance.queue_text(1, header(), "queue").unwrap();
    assert_eq!(
        instance.complete_tx(2, 1, identity),
        Err(InstanceError::CompletionBeforeDispatch {
            operation_id: 1,
            identity
        })
    );
    assert_eq!(instance.pending_identity(), Some(identity));
    assert_eq!(instance.take_outbound(2).unwrap().unwrap().operation_id, 1);
}

#[test]
fn in_flight_loss_is_refused_without_mutation() {
    let mut instance = instance();
    let identity = instance.queue_text(1, header(), "sent").unwrap();
    let outbound = instance.take_outbound(1).unwrap().unwrap();
    assert_eq!(
        instance.discard_pending(2, LossPermission::operator()),
        Err(InstanceError::InFlightRequiresSettlement {
            operation_id: outbound.operation_id,
            identity
        })
    );
    assert_eq!(instance.pending_identity(), Some(identity));
    assert_eq!(instance.pending_operation_id(), Some(outbound.operation_id));
}

#[test]
fn dispatched_failure_settles_late_but_queued_failure_is_refused() {
    let mut instance = instance();
    let identity = instance.queue_text(1, header(), "queue").unwrap();
    assert_eq!(
        instance.fail_tx(2, 1, identity),
        Err(InstanceError::CompletionBeforeDispatch {
            operation_id: 1,
            identity
        })
    );
    let outbound = instance.take_outbound(2).unwrap().unwrap();
    assert_eq!(
        instance.fail_tx(12, outbound.operation_id, outbound.identity),
        Ok(InstanceEvent::PendingLost {
            operation_id: outbound.operation_id,
            identity
        })
    );
    assert_eq!(instance.pending_identity(), None);
    assert_eq!(instance.next_deadline(), None);
}

#[test]
fn absence_keeps_directory_and_advances_the_instance_clock() {
    let mut instance = instance();
    let record = [
        0x22, 0x12, 0x08, 0x01, 0x12, 0x0e, 0x0a, 0x02, b'i', b'd', 0x12, 0x04, b'n', b'a', b'm',
        b'e', 0x1a, 0x02, b'n', b'm',
    ];
    assert!(instance.ingest_node_info(1, &record).unwrap());
    instance.pause(2, 2).unwrap();
    assert_eq!(instance.resume(20).unwrap(), None);
    assert_eq!(instance.directory().len(), 1);
    assert_eq!(
        instance.queue_text(19, header(), "old"),
        Err(InstanceError::TimeRegressed {
            previous: 20,
            now: 19
        })
    );
}
