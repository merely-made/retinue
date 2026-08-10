#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use retinue::node::{Action, Actions, InterfaceId, Node};
use retinue::{AddressHash, DestinationName, Packet, PrivateIdentity};

const IFACE: InterfaceId = 7;
const MAX_STEPS: usize = 32;
const FIXTURE_ANNOUNCE: &[u8] =
    include_bytes!("../../crates/retinue/tests/fixtures/announce_appdata.bin");

fn node(secret: u8, part: &'static str) -> Node {
    let identity = PrivateIdentity::from_secret_bytes(&[secret; 64]);
    let name = DestinationName::new("retinue", ["fuzz", part]);
    Node::new(identity, name.name_hash())
}

fn sent<const N: usize>(actions: Actions<N>) -> Option<Packet> {
    actions.into_iter().find_map(|action| match action {
        Action::Send { packet, .. } => Some(packet),
        _ => None,
    })
}

fn linked<const PEERS: usize, const ACTIONS: usize, const LINKS: usize>(
    receiver: &mut Node<PEERS, ACTIONS, LINKS>,
    sender: &mut Node<PEERS, ACTIONS, LINKS>,
) -> Option<AddressHash> {
    receiver.ingest(IFACE, &sender.announce(&[0xA1; 10], None), 0);
    let request = sent(receiver.open_link(sender.destination(), IFACE, &[0xB2; 64])?)?;
    let proof = sent(sender.ingest(IFACE, &request, 1))?;
    receiver
        .ingest(IFACE, &proof, 2)
        .into_iter()
        .find_map(|action| match action {
            Action::LinkUp { link_id } => Some(link_id),
            _ => None,
        })
}

fn mutate(frame: &mut [u8], bytes: &[u8]) {
    if frame.is_empty() {
        return;
    }
    for (index, byte) in bytes.iter().take(12).enumerate() {
        let offset = (usize::from(*byte) + index * 31) % frame.len();
        frame[offset] ^= byte.rotate_left((index % 8) as u32);
    }
}

fuzz_target!(|input: &[u8]| {
    // This starts every testcase from the same identity, clock, address book, and live link.
    // It therefore exercises the board ingress route rather than host randomness or task
    // scheduling. Failure input stays directly reproducible by cargo-fuzz's copied corpus.
    let mut receiver = node(0x11, "receiver");
    let mut sender = node(0x22, "sender");
    let Some(link_id) = linked(&mut receiver, &mut sender) else {
        return;
    };

    for (step, chunk) in input.chunks(48).take(MAX_STEPS).enumerate() {
        let Some((&selector, controls)) = chunk.split_first() else {
            continue;
        };
        let mut wire = match selector & 0b11 {
            // Raw bytes pressure the frame decoder directly.
            0 => controls.to_vec(),
            // A real announce means fuzzing reaches signature and address-book admission.
            1 => FIXTURE_ANNOUNCE.to_vec(),
            // A live encrypted link packet reaches link data and resource context dispatch.
            2 => sender
                .send(link_id, IFACE, controls, &[0xC3; 16])
                .and_then(sent)
                .map(|packet| packet.encode())
                .unwrap_or_default(),
            // Begin from a valid local announce so mutations preserve useful structure.
            _ => sender.announce(&[0xD4; 10], None).encode(),
        };
        // The first step remains unmodified to guarantee every corpus entry reaches a valid
        // ingress branch; later steps progressively alter trusted packet shapes.
        if step != 0 {
            mutate(&mut wire, controls);
        }
        if let Ok(packet) = Packet::decode(&wire) {
            let _ = receiver.ingest(IFACE, &packet, step as u64 + 3);
        }
    }
});
