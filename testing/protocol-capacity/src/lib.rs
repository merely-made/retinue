//! Concrete protocol capacities for compiler layout and allocation measurements.
//! This fixture is not installed firmware and contains no scheduling runtime.
#![no_std]

extern crate alloc;
use alloc::{vec, vec::Vec};

use core::mem::size_of;
use retinue::node::{FreshnessPolicy, Node, PayloadLimits};

pub type RetinueNode = Node<8, 4, 1, 4>;

pub const RETINUE_LIMITS: PayloadLimits = PayloadLimits {
    max_ingress_bytes: 255,
    max_app_data: 32,
    max_link_payload: 160,
    max_outbound_resource: 1024,
    max_resource_parts: 8,
};

pub fn retinue_node(seed: u8) -> RetinueNode {
    RetinueNode::new_with_payload_limits(
        retinue::identity::PrivateIdentity::from_secret_bytes(&[seed; 64]),
        retinue::destination::DestinationName::new("retinue", ["capacity"]).name_hash(),
        RETINUE_LIMITS,
    )
    .with_freshness_policy(FreshnessPolicy {
        max_destinations: 8,
        max_blobs_per_destination: 2,
        retention: 604_800_000,
    })
    .expect("nonzero fixture capacities")
}

// A stable symbol that can be read from target LLVM IR without executing target
// code. Values come from the target compiler, never from host pointer sizes.
#[unsafe(no_mangle)]
pub static PROTOCOL_LAYOUT_V1: [u32; 12] = [
    size_of::<usize>() as u32,
    size_of::<RetinueNode>() as u32,
    size_of::<retinue::node::Actions<4>>() as u32,
    size_of::<retinue::node::InterruptionReport<1, 4>>() as u32,
    size_of::<sennet::node::Channel>() as u32,
    size_of::<sennet::packet_id::PacketIdState>() as u32,
    size_of::<sennet::flood::ManagedFlood>() as u32,
    size_of::<sennet::node_info::NodeDirectory>() as u32,
    size_of::<tucket::node::Node>() as u32,
    size_of::<tucket::node::PendingText>() as u32,
    size_of::<Residents>() as u32,
    size_of::<tucket::node::PendingTexts>() as u32,
];

/// Persistent objects measured together. No protocol suspension is implied.
pub struct Residents {
    pub retinue: RetinueNode,
    pub channel: sennet::node::Channel,
    pub ids: sennet::packet_id::PacketIdState,
    pub flood: sennet::flood::ManagedFlood,
    pub directory: sennet::node_info::NodeDirectory,
    pub tucket: tucket::node::Node,
    pub pending: tucket::node::PendingTexts,
}

fn sends(actions: retinue::node::Actions<4>) -> Vec<retinue::Packet> {
    assert_eq!(actions.overflowed(), 0);
    actions
        .into_iter()
        .filter_map(|action| match action {
            retinue::node::Action::Send { packet, .. } => Some(packet),
            _ => None,
        })
        .collect()
}

/// Repeatable synthetic allocation workload, with real packet processing.
/// Seeds and keys are fixtures. Returned objects retain full directories and
/// pending work. Temporary oracle peers are included in the measured peak.
pub fn workload() -> Residents {
    use sennet::flood::{ManagedFlood, ManagedFloodConfig, RelayDelayWindow};
    use sennet::node_info::{NodeDirectory, NodeDirectoryConfig};
    use tucket::node::{NodeCapacity, PendingTexts, TextRetryPolicy};
    let mut state = Residents {
        retinue: retinue_node(1),
        channel: sennet::node::Channel {
            hash: 8,
            key: sennet::transport::ChannelKey::Aes128([1; 16]),
        },
        ids: sennet::packet_id::PacketIdState::new(1, 1),
        flood: ManagedFlood::new(ManagedFloodConfig {
            channel_hash: 8,
            relay_node: 2,
            seen_capacity: 32,
            delay: RelayDelayWindow::new(
                core::time::Duration::ZERO,
                core::time::Duration::from_millis(10),
            )
            .unwrap(),
        })
        .unwrap(),
        directory: NodeDirectory::with_config(NodeDirectoryConfig {
            capacity: 8,
            id_limit: 32,
            long_name_limit: 64,
            short_name_limit: 16,
        })
        .unwrap(),
        tucket: tucket::node::Node::with_capacity(
            tucket::identity::LocalIdentity::from_seed([1; 32]),
            false,
            NodeCapacity::new(8, 32).unwrap(),
        )
        .unwrap(),
        pending: PendingTexts::new(2).unwrap(),
    };

    // Full Sennet directory, plus churn beyond the dedup ring capacity.
    for number in 1..=8 {
        let mut user = Vec::new();
        for (tag, len) in [(1, 32), (2, 64), (3, 16)] {
            sennet::protobuf::write_tag(tag, 2, &mut user);
            sennet::protobuf::write_varint(len as u64, &mut user);
            user.extend(core::iter::repeat_n(b'x', len));
        }
        let mut info = vec![8, number];
        sennet::protobuf::write_tag(2, 2, &mut info);
        sennet::protobuf::write_varint(user.len() as u64, &mut info);
        info.extend(user);
        let mut outer = vec![34];
        sennet::protobuf::write_varint(info.len() as u64, &mut outer);
        outer.extend(info);
        assert!(state.directory.ingest_from_radio(&outer).unwrap());
    }
    let text = core::str::from_utf8(&[b'x'; 232]).unwrap();
    for _ in 0..128 {
        let identity = state.ids.reserve().unwrap();
        let header = sennet::transport::Header {
            destination: u32::MAX,
            source: identity.source,
            packet_id: identity.packet_id,
            hop_limit: 3,
            want_ack: false,
            via_mqtt: false,
            hop_start: 3,
            channel_hash: 8,
            next_hop: 0,
            relay_node: 1,
        };
        let frame = state.channel.seal_text(header, text).unwrap();
        assert_eq!(state.channel.open_text(&frame).unwrap().unwrap().text, text);
        let _ = state.flood.consider(&frame).unwrap();
    }
    assert_eq!(state.flood.seen_len(), 32);

    // Full Tucket contacts, maximum stored paths and two four-attempt texts.
    let mut hashes = Vec::with_capacity(8);
    for seed in 2..=100 {
        let mut peer = tucket::node::Node::with_capacity(
            tucket::identity::LocalIdentity::from_seed([seed; 32]),
            false,
            NodeCapacity::new(1, 1).unwrap(),
        )
        .unwrap();
        let hash = peer.my_hash();
        if hash == state.tucket.my_hash() || hashes.contains(&hash) {
            continue;
        }
        let frame = peer.advert_frame(1, b"fixture");
        let _ = state.tucket.on_frame(&frame);
        assert!(state.tucket.contact(hash).is_some());
        assert!(
            state
                .tucket
                .set_route(hash, tucket::node::DirectRoute::new(63, &[7; 63]).unwrap())
        );
        hashes.push(hash);
        if hashes.len() == 8 {
            break;
        }
    }
    assert_eq!(hashes.len(), 8);
    let text = core::str::from_utf8(&[b'x'; 171]).unwrap();
    for &hash in &hashes[..2] {
        let mut pending = state
            .tucket
            .try_begin_text(hash, 1, text, TextRetryPolicy::default())
            .unwrap();
        for _ in 0..4 {
            assert!(state.tucket.next_text_attempt(&mut pending).is_some());
        }
        state.pending.push(pending).unwrap();
    }

    // Fill Retinue peer/freshness rows, then retain a real link and resource.
    for seed in 2..=9 {
        let peer = retinue_node(seed).with_app_data(&[b'x'; 32]);
        for timebase in 1..=2 {
            let blob = retinue::announce::AnnounceBlob::mint([seed; 5], timebase).unwrap();
            state
                .retinue
                .ingest(0, &peer.announce(&blob, None), timebase);
        }
    }
    assert_eq!(state.retinue.peers().len(), 8);
    let mut peer = retinue_node(2);
    let request = sends(
        state
            .retinue
            .open_link(peer.destination(), 0, &[3; 64])
            .unwrap(),
    )
    .remove(0);
    let proof = sends(peer.ingest(0, &request, 3)).remove(0);
    let linked = state.retinue.ingest(0, &proof, 3);
    let id = linked
        .iter()
        .find_map(|a| match a {
            retinue::node::Action::LinkUp { link_id } => Some(*link_id),
            _ => None,
        })
        .unwrap();
    let data = [0x5a; 1024];
    let mut from_peer = sends(peer.publish(id, 0, &data, [5; 4], &[6; 16], 4).unwrap());
    let mut delivered = false;
    for now in 5..32 {
        let mut requests = Vec::new();
        for frame in from_peer {
            let actions = state.retinue.ingest(0, &frame, now);
            for a in actions.iter() {
                if let retinue::node::Action::Resource { data: body, .. } = a {
                    assert_eq!(body.as_slice(), data);
                    delivered = true;
                }
            }
            requests.extend(sends(actions));
        }
        from_peer = Vec::new();
        for frame in requests {
            from_peer.extend(sends(peer.ingest(0, &frame, now)));
        }
        if delivered {
            break;
        }
    }
    assert!(delivered);
    let _outbound = state
        .retinue
        .publish(id, 0, &data, [3; 4], &[4; 16], 33)
        .unwrap();
    assert!(state.retinue.transfer_active(id)); // Our outbound offer remains pending.
    state
}
