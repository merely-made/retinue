//! Ratchet rotation, persistence, and retained epochs.

use super::*;

fn advertised_ratchet_id(packet: &Packet) -> NameHash {
    let ratchet = Announce::decode(packet)
        .expect("locally emitted announce verifies")
        .ratchet
        .expect("a ratcheted destination announces its ratchet");
    NameHash::of(&ratchet)
}

/// Register a ratcheted destination whose current epoch was minted at `created_at`.
fn register_ratcheted_at(ep: &Endpoint, name: &DestinationName, created_at: u64) -> AddressHash {
    let dest = name.destination_hash(ep.identity());
    let mut store = crate::ratchet::RatchetStore::new(Default::default()).unwrap();
    store
        .rotate_if_due([0x41; KEY_LEN], created_at as f64)
        .unwrap();
    ep.shared.registered.lock().unwrap().push(Registered {
        dest,
        kind: RegistrationKind::Resource,
        name: name.clone(),
        app_data: b"ratchet".to_vec(),
        ratchets: Some(Arc::new(store)),
        enforce_ratchets: false,
        proof_strategy: ProofStrategy::None,
    });
    dest
}

const RATCHET_INTERVAL: u64 = 30 * 60;

#[tokio::test]
async fn announces_and_path_responses_rotate_a_due_ratchet() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x93; 64]));
    let name = DestinationName::new("retinue", ["ratchet-rotation"]);
    let dest = register_ratcheted_at(&ep, &name, 10_000);
    let first = ep.current_ratchet_id(&name).unwrap();

    // Until the interval has passed, every announce carries the same epoch.
    let held = ep.build_announce_at(&name, b"ratchet", 10_000 + RATCHET_INTERVAL);
    assert_eq!(advertised_ratchet_id(&held), first);

    // Past it, the announce rotates before it is built.
    let rotated = ep.build_announce_at(&name, b"ratchet", 10_001 + RATCHET_INTERVAL);
    let second = advertised_ratchet_id(&rotated);
    assert_ne!(second, first);
    assert_eq!(ep.current_ratchet_id(&name), Some(second));

    // A path response is an announce, so it rotates too.
    let response = ep
        .shared
        .path_response_at(dest, 10_002 + 2 * RATCHET_INTERVAL)
        .expect("owned destination answers a path request");
    let third = advertised_ratchet_id(&response);
    assert_ne!(third, second);
    assert_eq!(ep.current_ratchet_id(&name), Some(third));
    assert_eq!(ep.shared.registered_ratchets(dest).unwrap().len(), 3);
}

#[tokio::test]
async fn a_rotated_ratchet_is_persisted_before_it_is_advertised() {
    let identity = PrivateIdentity::from_secret_bytes(&[0x94; 64]);
    let ep = Endpoint::new(identity.clone());
    let name = DestinationName::new("retinue", ["ratchet-persist"]);
    let dest = register_ratcheted_at(&ep, &name, 10_000);
    let first = ep.current_ratchet_id(&name).unwrap();

    type Persisted = Vec<(Vec<u8>, Option<NameHash>)>;
    let persisted: Arc<Mutex<Persisted>> = Arc::default();
    let refuse = Arc::new(std::sync::atomic::AtomicBool::new(true));
    ep.set_ratchet_persistence({
        let shared = Arc::downgrade(&ep.shared);
        let persisted = Arc::clone(&persisted);
        let refuse = Arc::clone(&refuse);
        move |persisted_dest, snapshot| {
            assert_eq!(persisted_dest, dest);
            // What the endpoint would advertise while the host is still persisting.
            let installed = shared
                .upgrade()
                .and_then(|shared| shared.registered_ratchets(persisted_dest))
                .and_then(|store| store.current_id());
            persisted
                .lock()
                .unwrap()
                .push((snapshot.to_vec(), installed));
            if refuse.load(Ordering::SeqCst) {
                Err(io::Error::other("disk full"))
            } else {
                Ok(())
            }
        }
    });

    // A snapshot the host could not persist is never advertised.
    let later = 10_001 + RATCHET_INTERVAL;
    let refused = ep.build_announce_at(&name, b"ratchet", later);
    assert_eq!(advertised_ratchet_id(&refused), first);
    assert_eq!(ep.current_ratchet_id(&name), Some(first));

    refuse.store(false, Ordering::SeqCst);
    let accepted = ep.build_announce_at(&name, b"ratchet", later);
    let second = advertised_ratchet_id(&accepted);
    assert_ne!(second, first);

    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.len(), 2, "one refused and one accepted rotation");
    let (snapshot, installed_while_persisting) = &persisted[1];
    assert_eq!(
        *installed_while_persisting,
        Some(first),
        "the new epoch was persisted before it was installed or advertised"
    );
    let (restored, _) = crate::ratchet::RatchetStore::restore(
        Default::default(),
        snapshot,
        identity.public(),
        later as f64,
    )
    .unwrap();
    assert_eq!(restored.current_id(), Some(second));
    assert_eq!(restored.len(), 2);
}

#[tokio::test]
async fn an_epoch_superseded_at_announce_still_decrypts() {
    let identity = PrivateIdentity::from_secret_bytes(&[0x95; 64]);
    let ep = Endpoint::new(identity.clone());
    let name = DestinationName::new("retinue", ["ratchet-retained"]);
    let dest = register_ratcheted_at(&ep, &name, 10_000);
    let first = ep.current_ratchet_id(&name).unwrap();
    let old_public = ep
        .shared
        .registered_ratchets(dest)
        .and_then(|store| store.current_public())
        .unwrap();
    ep.build_announce_at(&name, b"ratchet", 10_001 + RATCHET_INTERVAL);
    assert_ne!(ep.current_ratchet_id(&name), Some(first));

    let payload = crate::token::encrypt_to_ratchet(
        identity.public(),
        &old_public,
        &[0x17; KEY_LEN],
        &[0x18; IV_LEN],
        b"older epoch",
    );
    let packet = Packet {
        ifac: false,
        header_type: crate::packet::HeaderType::Type1,
        context_flag: false,
        propagation: crate::packet::Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: dest,
        context: 0,
        payload,
    };
    deliver_single(&ep.shared, 1, &packet, packet.full_hash());
    let received = ep.accept_single().await.unwrap();
    assert_eq!(received.data, b"older epoch");
    assert_eq!(received.ratchet_id, Some(first));
}
