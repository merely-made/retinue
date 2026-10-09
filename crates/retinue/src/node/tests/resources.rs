use super::*;

/// Drive every packet between two nodes until neither has anything more to say.
///
/// This is the desk stand-in for a radio: it carries whatever each side wants sent to
/// the other, in order, with no loss. What it proves is that the two halves of a
/// transfer agree; loss and retransmission are the medium's business and are measured
/// on real hardware at the gates.
fn pump(
    a: &mut Node<32, 8, 4>,
    b: &mut Node<32, 8, 4>,
    first: Actions<8>,
) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut to_b: Vec<Packet> = first
        .iter()
        .filter_map(|x| match x {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        })
        .collect();
    let mut to_a: Vec<Packet> = Vec::new();
    let (mut got_a, mut got_b) = (Vec::new(), Vec::new());

    for _ in 0..64 {
        if to_a.is_empty() && to_b.is_empty() {
            break;
        }
        let (mut next_a, mut next_b) = (Vec::new(), Vec::new());

        for packet in to_b.drain(..) {
            for action in b.ingest(IFACE, &packet, 0) {
                match action {
                    Action::Send { packet, .. } => next_a.push(packet),
                    Action::Resource { data, .. } => got_b.push(data),
                    _ => {}
                }
            }
        }
        for packet in to_a.drain(..) {
            for action in a.ingest(IFACE, &packet, 0) {
                match action {
                    Action::Send { packet, .. } => next_b.push(packet),
                    Action::Resource { data, .. } => got_a.push(data),
                    _ => {}
                }
            }
        }
        to_a = next_a;
        to_b = next_b;
    }
    (got_a, got_b)
}

/// A resource crosses a link whole, reassembled and hash-verified.
///
/// Multi-part on purpose: one part would not exercise the request window, the hashmap,
/// or reassembly, which is where the interesting failures live.
#[test]
fn a_resource_crosses_a_link_whole() {
    let (mut a, mut b, id) = linked();
    let payload: Vec<u8> = (0..3_000u32).map(|i| (i.wrapping_mul(31)) as u8).collect();

    let started = a
        .publish(
            id,
            IFACE,
            &payload,
            [0xAB; 4],
            &[5; crate::token::IV_LEN],
            0,
        )
        .expect("a holds the link, so it can publish");
    assert!(a.transfer_active(id), "the transfer is running");

    let (_, got_b) = pump(&mut a, &mut b, started);

    assert_eq!(got_b.len(), 1, "b received exactly one resource");
    assert_eq!(got_b[0], payload, "byte for byte");
    assert!(!b.transfer_active(id), "and b cleared its receiver");
}

/// An advertisement past the node's part ceiling is refused, and nothing is held.
///
/// The sender picks the advertised size, so this is the point where a peer's ambition
/// stops being the board's problem. Without it a peer could name a resource far larger
/// than the board's memory and the board would try. The refusal goes on the wire, as
/// RNS rejects an offer, so the sender stops rather than re-advertising.
#[test]
fn an_oversized_resource_is_refused_without_holding_state() {
    let (mut a, mut b, id) = linked();

    // Comfortably past MAX_RESOURCE_PARTS even when compression is enabled.
    // The old repeating-byte fixture compressed below the advertised ceiling.
    let huge: Vec<u8> = (0..2_500u32)
        .flat_map(|i| crate::hash::full_hash(&i.to_le_bytes()))
        .collect();
    let started = a
        .publish(id, IFACE, &huge, [0xCD; 4], &[6; crate::token::IV_LEN], 0)
        .expect("a will happily offer it");

    let advertisement = sent(&started).expect("an advertisement goes out");
    let answer = b.ingest(IFACE, &advertisement, 0);

    let refusal = sent(&answer).expect("b rejects the offer");
    assert_eq!(
        refusal.context,
        link::CTX_RESOURCE_RCL,
        "with a receiver cancel"
    );
    assert_eq!(b.refused_offers(), 1);
    assert!(!b.transfer_active(id), "and holds no reassembly state");
    assert!(b.has_link(id), "while the link itself is untouched");

    assert!(a.ingest(IFACE, &refusal, 1).is_empty());
    assert!(!a.transfer_active(id), "the sender stops on the rejection");
}

/// Run a transfer from `a` to `b` until `b` proves receipt, returning that proof
/// undelivered. `a`'s sender is still waiting for it.
fn transfer_until_proof(a: &mut Node<32, 8, 4>, b: &mut Node<32, 8, 4>, id: AddressHash) -> Packet {
    let payload: Vec<u8> = (0..2_000u32).map(|i| (i.wrapping_mul(13)) as u8).collect();
    let started = a
        .publish(
            id,
            IFACE,
            &payload,
            [0x5A; 4],
            &[8; crate::token::IV_LEN],
            0,
        )
        .unwrap();
    let mut to_b = vec![sent(&started).unwrap()];
    for _ in 0..64 {
        let mut to_a = Vec::new();
        for packet in to_b.drain(..) {
            for action in b.ingest(IFACE, &packet, 0) {
                if let Action::Send { packet, .. } = action {
                    to_a.push(packet);
                }
            }
        }
        if let Some(index) = to_a
            .iter()
            .position(|p| p.context == link::CTX_RESOURCE_PRF)
        {
            return to_a.swap_remove(index);
        }
        for packet in to_a {
            for action in a.ingest(IFACE, &packet, 0) {
                if let Action::Send { packet, .. } = action {
                    to_b.push(packet);
                }
            }
        }
    }
    panic!("b never proved receipt");
}

/// The packets a set of actions wants sent with link context `context`.
fn sent_with<const N: usize>(actions: &Actions<N>, context: u8) -> Vec<Packet> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Send { packet, .. } if packet.context == context => Some(packet.clone()),
            _ => None,
        })
        .collect()
}

/// A lost resource proof is recovered as RNS recovers it: after a quiet retry interval
/// the sender asks for the proof by its packet hash, and the receiver answers from the
/// proof it kept, every time it is asked, rather than the whole transfer running again.
#[test]
fn a_lost_resource_proof_is_recovered_from_the_receivers_cache() {
    let (mut a, mut b, id) = linked();
    let proof = transfer_until_proof(&mut a, &mut b, id);
    assert!(
        !b.transfer_active(id),
        "b delivered and released its receiver"
    );

    let polled = a.poll(RESOURCE_RETRY_INTERVAL, IFACE, None);
    let [request] = sent_with(&polled, link::CTX_CACHE_REQUEST)
        .try_into()
        .expect("one cache request");
    assert!(
        sent_with(&polled, link::CTX_RESOURCE_ADV).is_empty(),
        "not a re-offer"
    );
    assert_eq!(request.payload, proof.full_hash());

    for _ in 0..2 {
        let answer = b.ingest(IFACE, &request, RESOURCE_RETRY_INTERVAL);
        assert_eq!(
            sent(&answer),
            Some(proof.clone()),
            "the kept proof, byte for byte"
        );
    }
    assert!(a.ingest(IFACE, &proof, RESOURCE_RETRY_INTERVAL).is_empty());
    assert!(!a.transfer_active(id), "the proof completed a's publish");

    // The kept proof expires.
    b.poll(
        RESOURCE_RETRY_INTERVAL + RESOURCE_PROOF_CACHE_TTL,
        IFACE,
        None,
    );
    assert!(
        b.ingest(IFACE, &request, RESOURCE_PROOF_CACHE_TTL * 2)
            .is_empty()
    );
}

/// A sender whose proof never comes asks three times, then cancels with a sealed
/// initiator cancel and lets the transfer go.
#[test]
fn a_sender_without_a_proof_cancels_after_three_cache_requests() {
    let (mut a, mut b, id) = linked();
    let proof = transfer_until_proof(&mut a, &mut b, id);
    // The peer is alive and answers keepalives; only the proof was lost. Without its
    // answers the link would go stale and close before the retries ran out.
    let peer_link = b
        .links
        .iter()
        .find(|(link, _, _)| link.id() == id)
        .unwrap()
        .0
        .clone();
    let mut now = 0;
    for _ in 0..crate::resource_transfer::PROOF_CACHE_REQUESTS {
        now += RESOURCE_RETRY_INTERVAL;
        a.ingest(
            IFACE,
            &peer_link.keepalive_packet(link::KEEPALIVE_RESPONSE),
            now - 1,
        );
        let polled = a.poll(now, IFACE, None);
        assert_eq!(sent_with(&polled, link::CTX_CACHE_REQUEST).len(), 1);
    }
    now += RESOURCE_RETRY_INTERVAL;
    a.ingest(
        IFACE,
        &peer_link.keepalive_packet(link::KEEPALIVE_RESPONSE),
        now - 1,
    );
    let polled = a.poll(now, IFACE, None);
    let [cancel] = sent_with(&polled, link::CTX_RESOURCE_ICL)
        .try_into()
        .expect("one initiator cancel");
    let (resource_hash, _) = crate::resource::parse_proof(&proof.payload).unwrap();
    let link = &b
        .links
        .iter()
        .find(|(link, _, _)| link.id() == id)
        .unwrap()
        .0;
    assert_eq!(link.decrypt(&cancel).unwrap(), resource_hash);
    assert!(!a.transfer_active(id));
}

/// The kept proof is re-sent a bounded number of times. A cache request is unencrypted
/// and anyone who heard the proof can name its hash, so without the cap a third party
/// could make the receiver transmit the proof for as long as it is kept.
#[test]
fn cache_request_answers_are_capped() {
    let (mut a, mut b, id) = linked();
    let proof = transfer_until_proof(&mut a, &mut b, id);
    let request = sent_with(
        &a.poll(RESOURCE_RETRY_INTERVAL, IFACE, None),
        link::CTX_CACHE_REQUEST,
    )
    .pop()
    .expect("a cache request");
    let answered = (0..20)
        .filter(|_| {
            sent(&b.ingest(IFACE, &request, RESOURCE_RETRY_INTERVAL)) == Some(proof.clone())
        })
        .count();
    assert_eq!(
        answered,
        usize::from(crate::resource_transfer::PROOF_CACHE_ANSWERS)
    );
}

/// A sender that lost the proof and offers the same resource again, as an older
/// retinue sender does instead of a cache request, is answered with the kept proof. The
/// resource is not received, or delivered, a second time.
#[test]
fn a_re_advertisement_of_a_proved_resource_is_answered_from_the_kept_proof() {
    let (mut a, mut b, id) = linked();
    let proof = transfer_until_proof(&mut a, &mut b, id);
    let mut counter = 0;
    let seed = [0x42; 64];
    let advertisement = a.senders[0]
        .1
        .advertisement(&derived_iv(&seed, id, &mut counter));
    let answer = b.ingest(IFACE, &advertisement, 1);
    assert_eq!(sent(&answer), Some(proof));
    assert!(
        !answer
            .iter()
            .any(|action| matches!(action, Action::Resource { .. })),
        "nothing delivered again"
    );
    assert!(!b.transfer_active(id), "and no transfer started");
}

/// A resource sent with metadata is delivered as its data alone, and the dropped
/// metadata is counted rather than lost silently.
#[test]
fn dropped_metadata_is_counted() {
    let (a, mut b, id) = linked();
    let link = a
        .links
        .iter()
        .find(|(link, _, _)| link.id() == id)
        .unwrap()
        .0
        .clone();
    let data = b"the data".to_vec();
    let mut sender = ResourceSender::publish_with_metadata(
        link,
        &data,
        &[0xA1, b'x'],
        [0x5B; 4],
        &[9; crate::token::IV_LEN],
    )
    .unwrap();
    let mut counter = 0;
    let seed = [0x43; 64];
    let mut to_b = vec![sender.advertisement(&derived_iv(&seed, id, &mut counter))];
    let mut delivered = None;
    for _ in 0..16 {
        let mut to_a = Vec::new();
        for packet in to_b.drain(..) {
            for action in b.ingest(IFACE, &packet, 0) {
                match action {
                    Action::Send { packet, .. } => to_a.push(packet),
                    Action::Resource { data, .. } => delivered = Some(data),
                    _ => {}
                }
            }
        }
        for packet in to_a {
            to_b.extend(sender.on_packet(&packet, || derived_iv(&seed, id, &mut counter)));
        }
    }
    assert_eq!(delivered, Some(data));
    assert!(sender.is_done());
    assert_eq!(b.dropped_metadata(), 1);
}

/// A Node proves a resource with the PROOF-type packet RNS accepts, and a Node sender
/// completes on one. Before, a PROOF-type packet only ever reached link setup, so a
/// Node publishing to RNS never saw its receipt and held the sender until it expired.
#[test]
fn a_proof_type_resource_proof_completes_a_node_sender() {
    let (mut a, mut b, id) = linked();
    let proof = transfer_until_proof(&mut a, &mut b, id);
    assert_eq!(proof.packet_type, PacketType::Proof);
    assert!(a.transfer_active(id), "a is still waiting for the receipt");

    assert!(a.ingest(IFACE, &proof, 0).is_empty());
    assert!(
        !a.transfer_active(id),
        "the PROOF-type receipt completes a's sender"
    );
    assert!(a.has_link(id));
}

/// For one release a Node sender still accepts the DATA-type proof older retinue sent.
#[test]
fn a_node_sender_still_accepts_the_legacy_data_type_proof() {
    let (mut a, mut b, id) = linked();
    let mut proof = transfer_until_proof(&mut a, &mut b, id);
    proof.packet_type = PacketType::Data;
    a.ingest(IFACE, &proof, 0);
    assert!(!a.transfer_active(id));
}

/// A PROOF-type resource proof on a link with no outbound transfer is dropped: it is
/// not an offer, so it neither opens a receiver nor counts as a refused one.
#[test]
fn a_stray_resource_proof_opens_nothing() {
    let (mut a, b, id) = linked();
    let link = b.links.iter().find(|(l, _, _)| l.id() == id).unwrap();
    let stray = link.0.resource_proof_packet(&[1; 32], &[2; 32]);
    assert!(a.ingest(IFACE, &stray, 0).is_empty());
    assert!(!a.transfer_active(id));
    assert_eq!(a.refused_offers(), 0);
}

/// A multi-segment offer is refused with a sealed cancel and holds no state, rather
/// than being received as its first segment.
#[test]
fn a_multi_segment_offer_is_refused_with_a_cancel() {
    let (a, mut b, id) = linked();
    let link = a
        .links
        .iter()
        .find(|(l, _, _)| l.id() == id)
        .unwrap()
        .0
        .clone();
    let segment = [0x42_u8; 600];
    let random_hash = [1, 2, 3, 4];
    let iv = [0x11; crate::token::IV_LEN];
    let token = link.seal(&crate::resource::content(&segment, &random_hash), &iv);
    let out = crate::resource::Outgoing::new(&segment, &token, random_hash, false).with_segment(
        1,
        3,
        1_800,
        crate::resource::resource_hash(&segment, &random_hash),
    );
    let advertisement =
        link.sealed_packet(link::CTX_RESOURCE_ADV, &out.advertisement().pack(), &iv);

    let answer = b.ingest(IFACE, &advertisement, 0);
    let cancel = sent(&answer).expect("b tells the sender to stop");
    assert_eq!(cancel.context, link::CTX_RESOURCE_RCL);
    assert_eq!(link.decrypt(&cancel).unwrap(), out.resource_hash().to_vec());
    assert!(
        !answer.iter().any(|x| matches!(x, Action::Resource { .. })),
        "no data"
    );
    assert!(!b.transfer_active(id), "and holds no reassembly state");
    assert_eq!(b.refused_offers(), 1);
}
