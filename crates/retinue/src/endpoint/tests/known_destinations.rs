//! Address-book persistence across a restart.

use super::*;

/// A ratcheted peer's announce, with the ratchet's secret half.
fn ratcheted_peer() -> (PrivateIdentity, [u8; KEY_LEN], Packet, Announce) {
    let peer = PrivateIdentity::from_secret_bytes(&[0xC2; 64]);
    let secret = [0xC3; KEY_LEN];
    let public = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(secret));
    let packet = announce::build(
        &peer,
        DestinationName::new("retinue", ["persisted"]).name_hash(),
        &AnnounceBlob::from_wire([0xC4; crate::announce::RAND_HASH_LEN]),
        Some(public.as_bytes()),
        b"persisted",
    );
    let decoded = Announce::decode(&packet).unwrap();
    (peer, secret, packet, decoded)
}

/// A restarted endpoint restores its signed address book and sends to a peer's received
/// ratchet without hearing it announce again (RNS `Identity.py` 177-240, 484-508). A
/// tampered or foreign snapshot is refused and restores nothing.
#[tokio::test]
async fn a_restarted_endpoint_sends_to_a_restored_peer() {
    let identity = PrivateIdentity::from_secret_bytes(&[0xC1; 64]);
    let (peer, ratchet_secret, packet, announced) = ratcheted_peer();
    let dest = announced.destination;

    let snapshot = {
        let ep = Endpoint::new(identity.clone());
        let wire = ep.attach_interface();
        process_verified_announce(&ep.shared, wire.id(), packet, announced);
        let persisted: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        ep.set_address_book_persistence({
            let persisted = Arc::clone(&persisted);
            move |snapshot| {
                persisted.lock().unwrap().push(snapshot.to_vec());
                Ok(())
            }
        });
        let cleaned = ep.persist_address_book().unwrap();
        assert_eq!(cleaned.removed, 0, "a peer with a live path is kept");
        ep.close();
        persisted
            .lock()
            .unwrap()
            .pop()
            .expect("the hook got a snapshot")
    };

    let ep = Endpoint::new(identity);
    let wire = ep.attach_interface();
    let unknown = ep.send_single(dest, b"hello").unwrap_err();
    assert_eq!(unknown.kind(), io::ErrorKind::NotFound);

    let mut tampered = snapshot.clone();
    tampered[20] ^= 0x01;
    let refused = ep.restore_address_book(&tampered).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
    let stranger = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0xC5; 64]));
    assert!(stranger.restore_address_book(&snapshot).is_err());
    assert!(
        ep.resolve(dest).is_none(),
        "a refused snapshot restores nothing"
    );

    assert_eq!(ep.restore_address_book(&snapshot).unwrap().loaded, 1);
    assert_eq!(ep.resolve(dest), Some(*peer.public()));
    ep.send_single(dest, b"hello").unwrap();
    let sent = wire.outbound.queues.pop().expect("the packet went out");
    let (plaintext, _) =
        crate::token::decrypt_with_ratchets(&peer, [&ratchet_secret], &sent.payload)
            .expect("encrypted to the restored ratchet");
    assert_eq!(plaintext, b"hello");
    let book = ep.shared.address_book.lock().unwrap();
    assert!(
        book.resolve(dest).unwrap().last_used > 0,
        "the send was a use"
    );
}
