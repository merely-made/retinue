use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::destination::DestinationName;
use crate::identity::PrivateIdentity;
use crate::link::{CTX_RESOURCE_PRF, Link, LinkMode, LinkTrailer, PendingLink, accept};
use crate::packet::Packet;
use crate::token::IV_LEN;

mod proof;
mod refusal;
mod segments;
mod transfer;

/// An established link between a sender side and a receiver side.
fn link_pair_with_mtu(mtu: u32) -> (Link, Link) {
    let server = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu,
    };
    let dest = DestinationName::new("retinue", ["res"]).destination_hash(server.public());
    let (pending, request) = PendingLink::open(dest, *server.public(), &[0x33; 64], trailer);
    let (recv_link, proof) = accept(&request, &server, &[0x99; 64], trailer).unwrap();
    let send_link = pending.prove(&proof).unwrap();
    (send_link, recv_link)
}

fn link_pair() -> (Link, Link) {
    link_pair_with_mtu(500)
}

fn iv_gen() -> impl FnMut() -> [u8; IV_LEN] {
    let mut n: u64 = 0;
    move || {
        n += 1;
        let mut v = [0u8; IV_LEN];
        v[..8].copy_from_slice(&n.to_le_bytes());
        v
    }
}

fn payload(len: usize) -> Vec<u8> {
    (0..len as u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect()
}

fn sha256_counter_payload(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut counter = 0_u64;
    while data.len() < len {
        data.extend_from_slice(&crate::hash::full_hash(&counter.to_be_bytes()));
        counter += 1;
    }
    data.truncate(len);
    data
}

/// Deliver `packets` to one side, collecting what it answers.
fn deliver(packets: Vec<Packet>, side: impl FnMut(&Packet) -> Vec<Packet>) -> Vec<Packet> {
    packets.iter().flat_map(side).collect()
}

/// Drive a clean transfer to the receiver's proof, returning the sender (not yet shown
/// the proof), the receiver, and the proof packet the receiver emitted.
fn transfer_until_proof(
    data: &[u8],
    ivg: &mut impl FnMut() -> [u8; IV_LEN],
) -> (ResourceSender, ResourceReceiver, Packet) {
    let (send_link, recv_link) = link_pair();
    let mut sender = ResourceSender::publish(send_link, data, [9, 8, 7, 6], &ivg());
    let mut receiver = ResourceReceiver::new(recv_link);
    let mut to_receiver = vec![sender.advertisement(&ivg())];
    for _ in 0..100 {
        let mut to_sender = Vec::new();
        for packet in core::mem::take(&mut to_receiver) {
            to_sender.extend(receiver.on_packet(&packet, &mut *ivg));
        }
        if let Some(index) = to_sender.iter().position(|p| p.context == CTX_RESOURCE_PRF) {
            return (sender, receiver, to_sender.swap_remove(index));
        }
        for packet in to_sender {
            to_receiver.extend(sender.on_packet(&packet, &mut *ivg));
        }
    }
    panic!("the receiver never proved");
}
