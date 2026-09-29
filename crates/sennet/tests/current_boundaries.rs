//! Current documented boundaries; project-generated tests, not versioned RF receipts.

use sennet::application::{ApplicationEnvelope, ApplicationError};
use sennet::flood::{
    FloodDecision, FloodIgnore, ManagedFlood, ManagedFloodConfig, RelayDelayWindow,
};
use sennet::instance::{PacketIdLease, ReceiveOutcome, SennetInstance, SennetInstanceConfig};
use sennet::node::Channel;
use sennet::node_info::NodeDirectoryConfig;
use sennet::transport::{BROADCAST_DESTINATION, ChannelKey, Header, MAX_PAYLOAD_LEN, Packet};
use std::time::Duration;

fn channel() -> Channel {
    Channel {
        hash: 8,
        key: ChannelKey::Aes128([7; 16]),
    }
}

fn flood_config() -> ManagedFloodConfig {
    ManagedFloodConfig {
        channel_hash: 8,
        relay_node: 4,
        seen_capacity: 2,
        delay: RelayDelayWindow::new(Duration::ZERO, Duration::ZERO).unwrap(),
    }
}

fn instance() -> SennetInstance {
    SennetInstance::new(
        SennetInstanceConfig {
            channel: channel(),
            flood: flood_config(),
            directory: NodeDirectoryConfig::default(),
            pending_ttl: 10,
        },
        PacketIdLease::new(0x0102_0304, 10, 13, 10).unwrap(),
    )
    .unwrap()
}

fn header(destination: u32) -> Header {
    Header {
        destination,
        source: 0x5566_7788,
        packet_id: 1,
        hop_limit: 3,
        want_ack: false,
        via_mqtt: false,
        hop_start: 3,
        channel_hash: 8,
        next_hop: 0,
        relay_node: 8,
    }
}

#[test]
fn every_port_varint_width_respects_the_complete_transport_budget() {
    for (port, limit) in [
        (1, 232),
        (128, 231),
        (16384, 230),
        (2097152, 229),
        (u32::MAX, 228),
    ] {
        let payload = vec![b'x'; limit];
        let encoded = ApplicationEnvelope::new(port, &payload).encode().unwrap();
        assert_eq!(encoded.len(), MAX_PAYLOAD_LEN);
        assert_eq!(
            ApplicationEnvelope::decode(&encoded).unwrap().payload,
            payload
        );
        assert_eq!(
            ApplicationEnvelope::new(port, &vec![0; limit + 1]).encode(),
            Err(ApplicationError::PayloadTooLong {
                actual: limit + 1,
                limit
            })
        );
    }
}

#[test]
fn broadcast_engine_leaves_directed_routing_to_its_owner() {
    let mut relay = ManagedFlood::new(flood_config()).unwrap();
    let directed = channel()
        .seal_text(header(0x1122_3344), "directed")
        .unwrap();
    assert_eq!(
        relay.consider(&directed).unwrap(),
        FloodDecision::Ignore(FloodIgnore::Directed)
    );
    assert_eq!(relay.seen_len(), 0);
    let broadcast = channel()
        .seal_text(header(BROADCAST_DESTINATION), "broadcast")
        .unwrap();
    assert!(matches!(
        relay.consider(&broadcast).unwrap(),
        FloodDecision::Relay { .. }
    ));
}

#[test]
fn leaf_does_not_deliver_or_remember_another_destinations_text() {
    let mut leaf = instance();
    let wrong = channel()
        .seal_text(header(0x1122_3344), "another node")
        .unwrap();
    assert_eq!(
        leaf.receive(1, &wrong).unwrap(),
        ReceiveOutcome::IgnoredDestination
    );
    let ours = channel().seal_text(header(0x0102_0304), "ours").unwrap();
    assert!(
        matches!(leaf.receive(2, &ours).unwrap(), ReceiveOutcome::Text(text) if text.text == "ours")
    );
    assert!(matches!(
        leaf.receive(3, &ours).unwrap(),
        ReceiveOutcome::Duplicate { .. }
    ));
}

#[test]
fn malformed_application_cannot_poison_valid_text_with_the_same_identity() {
    let mut leaf = instance();
    let mut malformed = Packet {
        header: header(BROADCAST_DESTINATION),
        payload: vec![0xff],
    };
    malformed.apply_channel_cipher(&channel().key);
    assert!(leaf.receive(1, &malformed.encode().unwrap()).is_err());
    let valid = channel()
        .seal_text(header(BROADCAST_DESTINATION), "valid")
        .unwrap();
    assert!(
        matches!(leaf.receive(2, &valid).unwrap(), ReceiveOutcome::Text(text) if text.text == "valid")
    );
}
