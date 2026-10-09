//! Endpoint unit tests, by topic.

mod announces;
mod freshness;
mod interfaces;
mod known_destinations;
mod packets;
mod path_requests;
mod ratchets;
mod rebroadcast;
mod reliable;
mod routes;
mod transit;

use alloc::vec;
use alloc::vec::Vec;

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::sync::oneshot;

use crate::address_book::AddressBook;
use crate::announce::{self, Announce, AnnounceBlob};
use crate::announce_admission::{AnnounceAdmission, AnnounceIngressPolicy, DestinationVerdict};
use crate::channel::StreamDecodeError;
use crate::destination::DestinationName;
use crate::hash::{AddressHash, NameHash};
use crate::identity::{KEY_LEN, PrivateIdentity};
use crate::ifac::Ifac;
use crate::link::{self, CTX_CHANNEL, CTX_LINKCLOSE, LinkMode, LinkTrailer};
use crate::link_liveness::Liveness;
use crate::node::InterfaceMode;
use crate::packet::{DestinationType, Packet, PacketType};
use crate::token::IV_LEN;

use super::announces::{AnnounceFreshnessPolicy, process_verified_announce};
use super::config::DEFAULT_LINK_MTU;
use super::dedup::{HashWindow, LinkPacketAdmission, LinkPacketMemory};
use super::facts::{LinkDirection, LinkRemoteFact};
use super::interface::{Iface, Interface, InterfaceId, QueueAdmission};
use super::paths::{
    PATH_REQUEST_GLOBAL_MAX, PATH_REQUEST_MIN_INTERVAL, PATH_TABLE_CAPACITY, PathEntry,
};
use super::queue::{OutboundQueues, QueueDepths, QueueWeights, TrafficClass};
use super::registration::{Registered, RegistrationKind};
use super::reliable_driver::register_reliable_stream;
use super::router::route;
use super::routing::MAX_HOPS;
use super::runtime::Endpoint;
use super::single::{ProofStrategy, SINGLE_RECEIPTS, SingleDelivery, deliver_single};
use super::stream::{LinkStream, register_stream};
use super::transit::{
    LINK_TRANSPORT_CAPACITY, LINK_TRANSPORT_IDLE, LINK_TRANSPORT_TTL, LinkBridge,
    REVERSE_TABLE_CAPACITY, REVERSE_TIMEOUT,
};

/// A peer announce, decoded, and the packet that carried it.
fn peer_announce(seed: u8, aspect: &str) -> (Packet, Announce) {
    let id = PrivateIdentity::from_secret_bytes(&[seed; 64]);
    let packet = announce::build(
        &id,
        DestinationName::new("retinue", [aspect]).name_hash(),
        &AnnounceBlob::from_wire([seed; crate::announce::RAND_HASH_LEN]),
        None,
        b"",
    );
    let decoded = Announce::decode(&packet).unwrap();
    (packet, decoded)
}

fn freshness_announce(
    peer: &PrivateIdentity,
    destination_name: &str,
    context: u8,
    nonce: u8,
    timebase: u64,
    hops: u8,
) -> (Packet, Announce) {
    let blob = AnnounceBlob::mint([nonce; crate::announce::ANNOUNCE_NONCE_LEN], timebase)
        .expect("test timebase fits");
    let name = DestinationName::new("retinue", [destination_name]);
    let mut packet = announce::build(peer, name.name_hash(), &blob, None, b"freshness-test");
    packet.context = context;
    packet.hops = hops;
    let decoded = Announce::decode(&packet).expect("locally built announce verifies");
    (packet, decoded)
}

/// A link-less data packet to `destination`, carrying `payload` as is.
fn single_packet(destination: AddressHash, payload: Vec<u8>) -> Packet {
    Packet {
        ifac: false,
        header_type: crate::packet::HeaderType::Type1,
        context_flag: false,
        propagation: crate::packet::Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination,
        context: 0,
        payload,
    }
}
