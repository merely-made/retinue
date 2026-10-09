use super::*;
use crate::identity::{Identity, LocalIdentity};
use crate::packet::{Packet, ROUTE_DIRECT};
use alloc::{vec, vec::Vec};

fn node(seed: u8, forward: bool) -> Node {
    Node::new(LocalIdentity::from_seed([seed; 32]), forward)
}

#[test]
fn configured_contact_and_pending_bounds_refuse_without_eviction() {
    let mut alice = Node::with_capacity(
        LocalIdentity::from_seed([0x10; 32]),
        false,
        NodeCapacity::new(1, 2).unwrap(),
    )
    .unwrap();
    let mut bob = node(0x20, false);
    let mut carol = node(0x30, false);
    let bob_advert = bob.advert_frame(1, b"bob");
    let carol_advert = carol.advert_frame(2, b"carol");
    assert!(matches!(
        alice.on_frame(&bob_advert).0.as_slice(),
        [Event::Advert { .. }]
    ));
    assert!(alice.contact(bob.my_hash()).is_some());
    assert!(
        alice.on_frame(&carol_advert).0.is_empty(),
        "a full node refuses a new contact"
    );
    assert!(
        alice.contact(bob.my_hash()).is_some(),
        "existing contact remains"
    );
    assert!(alice.contact(carol.my_hash()).is_none());
    let long_borrowed = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(matches!(
        alice.try_begin_text(bob.my_hash(), 3, &long_borrowed, TextRetryPolicy::default()),
        Err(CapacityError::TextTooLong)
    ));
    assert!(matches!(
        alice.try_begin_text(
            bob.my_hash(),
            3,
            "bounded",
            TextRetryPolicy {
                attempts: 5,
                flood_last: false
            },
        ),
        Err(CapacityError::InvalidRetryPolicy)
    ));
    let pending = alice
        .try_begin_text(bob.my_hash(), 3, "bounded", TextRetryPolicy::default())
        .unwrap();
    let mut outstanding = PendingTexts::new(1).unwrap();
    outstanding.push(pending.clone()).unwrap();
    assert_eq!(outstanding.push(pending), Err(CapacityError::PendingFull));
    assert!(matches!(
        alice.try_advert_frame(4, &[0; crate::advert::MAX_ADVERT_DATA + 1]),
        Err(CapacityError::AdvertDataTooLong)
    ));
}

#[test]
fn two_nodes_advert_then_message_and_ack() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    assert_ne!(
        alice.my_hash(),
        bob.my_hash(),
        "seeds must not collide on the 1-byte hash"
    );

    // Adverts both ways: each learns the other.
    let a_adv = alice.advert_frame(100, b"alice");
    let (ev, _) = bob.on_frame(&a_adv);
    assert!(matches!(ev.as_slice(), [Event::Advert { .. }]));
    assert!(bob.contact(alice.my_hash()).is_some(), "bob learned alice");

    let b_adv = bob.advert_frame(101, b"bob");
    alice.on_frame(&b_adv);
    assert!(alice.contact(bob.my_hash()).is_some(), "alice learned bob");

    // Alice sends bob a text; bob decrypts it and derives the matching ack.
    let (txt, expected_ack) = alice.text_frame(bob.my_hash(), 200, "hello bob").unwrap();
    let (ev, _) = bob.on_frame(&txt);
    let (from, message, ack) = match ev.as_slice() {
        [Event::Message { from, message, ack }] => (*from, message.clone(), *ack),
        other => panic!("expected a message, got {other:?}"),
    };
    assert_eq!(from, alice.my_hash());
    assert_eq!(message.text, "hello bob");
    assert_eq!(ack, expected_ack, "sender and receiver agree on the ack");

    // Bob acks; alice sees the ack she was awaiting.
    let ack_frame = bob.ack_frame(ack);
    let (ev, _) = alice.on_frame(&ack_frame);
    assert!(
        matches!(ev.as_slice(), [Event::Ack(a)] if *a == expected_ack),
        "alice matched her pending ack",
    );
}

#[test]
fn a_duplicate_flood_is_dropped() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    let adv = alice.advert_frame(1, b"a");
    let (first, _) = bob.on_frame(&adv);
    assert_eq!(first.len(), 1, "first sighting surfaces an advert");
    let (second, _) = bob.on_frame(&adv);
    assert!(second.is_empty(), "the duplicate is suppressed");
}

#[test]
fn operator_can_set_a_validated_route_for_a_known_contact() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    alice.on_frame(&bob.advert_frame(1, b"bob"));

    assert!(DirectRoute::new(2, &[0x33]).is_none());
    assert!(!alice.set_route(
        0xff,
        DirectRoute::new(1, &[0x33]).expect("valid one-hop route")
    ));
    assert!(alice.set_route(
        bob.my_hash(),
        DirectRoute::new(1, &[0x33]).expect("valid one-hop route")
    ));
    assert_eq!(
        alice.route_to(bob.my_hash()).expect("route stored").path(),
        &[0x33]
    );
}

#[test]
fn a_same_identity_refresh_keeps_the_learned_route() {
    let mut alice = node(0x11, false);
    let bob = node(0x22, false);
    let identity = bob.identity().clone();
    let hash = identity.hash()[0];

    alice.try_add_contact(identity.clone()).unwrap();
    assert!(alice.set_route(
        hash,
        DirectRoute::new(1, &[0x33]).expect("valid one-hop route")
    ));

    assert_eq!(alice.try_add_contact(identity.clone()), Ok(()));
    assert_eq!(alice.contact(hash), Some(&identity));
    assert_eq!(
        alice.route_to(hash).expect("route preserved").path(),
        &[0x33]
    );
}

#[test]
fn a_hash_collision_refuses_to_replace_contact_or_route() {
    let mut alice = node(0x11, false);
    let primary = Identity::new([0xa5; crate::packet::PUB_KEY_SIZE]);
    let mut colliding = primary.clone();
    colliding.pub_key[1] ^= 0xff;
    let hash = primary.hash()[0];

    alice.try_add_contact(primary.clone()).unwrap();
    assert!(alice.set_route(
        hash,
        DirectRoute::new(1, &[0x33]).expect("valid one-hop route")
    ));

    assert_eq!(
        alice.try_add_contact(colliding),
        Err(CapacityError::ContactHashCollision)
    );
    assert_eq!(alice.contact(hash), Some(&primary));
    assert_eq!(
        alice.route_to(hash).expect("route preserved").path(),
        &[0x33]
    );
}

#[test]
fn a_repeater_forwards_a_flood_it_is_not_the_target_of() {
    let mut alice = node(0x11, false);
    let mut repeater = node(0x33, true); // allow_forward
    let adv = alice.advert_frame(1, b"a");
    let (events, out) = repeater.on_frame(&adv);
    assert!(matches!(events.as_slice(), [Event::Advert { .. }]));
    assert_eq!(out.len(), 1, "the repeater re-floods the advert");
    // The re-flooded packet carries the repeater's hash appended to the path.
    let fwd = Packet::decode(&out[0]).unwrap();
    assert_eq!(fwd.path, vec![repeater.my_hash()]);
}

#[test]
fn a_leaf_does_not_forward() {
    let mut alice = node(0x11, false);
    let mut leaf = node(0x33, false); // no forwarding
    let adv = alice.advert_frame(1, b"a");
    let (_, out) = leaf.on_frame(&adv);
    assert!(out.is_empty(), "a leaf consumes without re-flooding");
}

#[test]
fn a_message_for_someone_else_is_not_decrypted() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    let mut carol = node(0x44, true); // a forwarding bystander
    alice.on_frame(&bob.advert_frame(1, b"b"));
    bob.on_frame(&alice.advert_frame(2, b"a"));
    carol.on_frame(&alice.advert_frame(3, b"a"));

    let (txt, _) = alice.text_frame(bob.my_hash(), 5, "for bob only").unwrap();
    // Carol is not the target: no Message event, but she forwards it.
    let (events, out) = carol.on_frame(&txt);
    assert!(!events.iter().any(|e| matches!(e, Event::Message { .. })));
    assert_eq!(
        out.len(),
        1,
        "carol forwards a message not addressed to her"
    );
}

#[test]
fn flood_message_establishes_reciprocal_direct_routes() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    let mut repeater = node(0x33, true);

    // Each endpoint learns the other's identity through the repeater.
    let a_adv = alice.advert_frame(1, b"alice");
    let (_, forwarded) = repeater.on_frame(&a_adv);
    bob.on_frame(&forwarded[0]);
    let b_adv = bob.advert_frame(2, b"bob");
    let (_, forwarded) = repeater.on_frame(&b_adv);
    alice.on_frame(&forwarded[0]);

    // The first text floods. Bob's generated PATH response carries its ACK and Alice's
    // outbound route inside the pairwise cipher.
    let (first, expected_ack) = alice.text_frame(bob.my_hash(), 3, "find a path").unwrap();
    assert!(Packet::decode(&first).unwrap().is_flood());
    let (_, forwarded) = repeater.on_frame(&first);
    let (events, bob_out) = bob.on_frame(&forwarded[0]);
    assert!(matches!(events.as_slice(), [Event::Message { .. }]));
    assert_eq!(bob_out.len(), 1, "bob emits a PATH response");

    let (_, forwarded) = repeater.on_frame(&bob_out[0]);
    let (events, alice_out) = alice.on_frame(&forwarded[0]);
    assert!(matches!(events.as_slice(), [Event::Ack(ack)] if *ack == expected_ack));
    assert_eq!(
        alice.route_to(bob.my_hash()).unwrap().path(),
        &[repeater.my_hash()]
    );
    assert_eq!(
        alice_out.len(),
        1,
        "alice returns the reciprocal path directly"
    );

    let (_, forwarded) = repeater.on_frame(&alice_out[0]);
    bob.on_frame(&forwarded[0]);
    assert_eq!(
        bob.route_to(alice.my_hash()).unwrap().path(),
        &[repeater.my_hash()]
    );

    // Later text and its addressed ACK use the learned source routes in both directions.
    let (second, ack) = alice.text_frame(bob.my_hash(), 4, "now direct").unwrap();
    let second = Packet::decode(&second).unwrap();
    assert_eq!(second.route_type(), ROUTE_DIRECT);
    assert_eq!(second.path, vec![repeater.my_hash()]);
    let (_, forwarded) = repeater.on_frame(&second.encode());
    let (events, _) = bob.on_frame(&forwarded[0]);
    assert!(
        matches!(events.as_slice(), [Event::Message { message, .. }] if message.text == "now direct")
    );

    let ack_frame = bob.ack_frame_to(alice.my_hash(), ack);
    assert_eq!(
        Packet::decode(&ack_frame).unwrap().route_type(),
        ROUTE_DIRECT
    );
    let (_, forwarded) = repeater.on_frame(&ack_frame);
    let (events, _) = alice.on_frame(&forwarded[0]);
    assert!(matches!(events.as_slice(), [Event::Ack(got)] if *got == ack));
}

fn establish_route(alice: &mut Node, bob: &mut Node, repeater: &mut Node) {
    let a_adv = alice.advert_frame(1, b"alice");
    let (_, forwarded) = repeater.on_frame(&a_adv);
    bob.on_frame(&forwarded[0]);
    let b_adv = bob.advert_frame(2, b"bob");
    let (_, forwarded) = repeater.on_frame(&b_adv);
    alice.on_frame(&forwarded[0]);

    let (first, _) = alice.text_frame(bob.my_hash(), 3, "learn route").unwrap();
    let (_, forwarded) = repeater.on_frame(&first);
    let (_, bob_out) = bob.on_frame(&forwarded[0]);
    let (_, forwarded) = repeater.on_frame(&bob_out[0]);
    let (_, alice_out) = alice.on_frame(&forwarded[0]);
    let (_, forwarded) = repeater.on_frame(&alice_out[0]);
    bob.on_frame(&forwarded[0]);
}

#[test]
fn direct_retries_clear_the_path_and_flood_the_last_attempt() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    let mut repeater = node(0x33, true);
    establish_route(&mut alice, &mut bob, &mut repeater);
    assert!(alice.route_to(bob.my_hash()).is_some());

    let mut pending = alice
        .begin_text(bob.my_hash(), 10, "retry me", TextRetryPolicy::default())
        .unwrap();
    let attempts: Vec<_> = (0..4)
        .map(|_| alice.next_text_attempt(&mut pending).unwrap())
        .collect();

    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt.attempt)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert!(attempts[..3].iter().all(|attempt| !attempt.flooded));
    assert!(attempts[3].flooded);
    assert!(alice.route_to(bob.my_hash()).is_none());
    assert_eq!(pending.attempts_remaining(), 0);
    assert!(alice.next_text_attempt(&mut pending).is_none());
}

#[test]
fn delayed_ack_from_any_attempt_completes_the_send() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    alice.on_frame(&bob.advert_frame(1, b"bob"));

    let mut pending = alice
        .begin_text(bob.my_hash(), 10, "eventually", TextRetryPolicy::default())
        .unwrap();
    let first = alice.next_text_attempt(&mut pending).unwrap();
    let second = alice.next_text_attempt(&mut pending).unwrap();
    assert_ne!(first.ack, second.ack, "attempt is covered by the ACK hash");
    assert!(pending.acknowledge(first.ack));
    assert!(pending.is_complete());
    assert!(alice.next_text_attempt(&mut pending).is_none());
}

#[test]
fn flood_fallback_can_be_disabled() {
    let mut alice = node(0x11, false);
    let mut bob = node(0x22, false);
    let mut repeater = node(0x33, true);
    establish_route(&mut alice, &mut bob, &mut repeater);

    let policy = TextRetryPolicy::new(2, false).unwrap();
    let mut pending = alice
        .begin_text(bob.my_hash(), 10, "stay direct", policy)
        .unwrap();
    assert!(!alice.next_text_attempt(&mut pending).unwrap().flooded);
    assert!(!alice.next_text_attempt(&mut pending).unwrap().flooded);
    assert!(alice.route_to(bob.my_hash()).is_some());
}
