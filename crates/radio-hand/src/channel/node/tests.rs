use super::*;

#[test]
fn far_future_heartbeat_does_not_jump_the_logical_lease() {
    // A node may be scheduled after any wall-clock or uptime value. Its
    // announce ordinal stays a one-per-attempt sequence, leaving a 65,536
    // entry boot lease useful for attempts rather than seconds of uptime.
    let mut timebase =
        TimebaseGenerator::firmware_lease(65_536, 131_072).expect("representable test lease");
    let blob = NodeChannel::<32, 8, 4>::next_announce_blob(
        &mut timebase,
        u64::MAX,
        [0xa5; ANNOUNCE_NONCE_LEN],
    )
    .expect("first attempted announce fits the lease");

    assert_eq!(blob.timebase(), 65_537);
    assert_eq!(timebase.last_emitted(), 65_537);
}

#[test]
fn non_due_beat_does_not_consume_an_ordinal() {
    let mut timebase = TimebaseGenerator::firmware_lease(12, 20).unwrap();
    let blob = NodeChannel::<32, 8, 4>::announce_blob_if_due(
        &mut timebase,
        false,
        u64::MAX,
        [0; ANNOUNCE_NONCE_LEN],
    )
    .expect("a non-due beat is valid");

    assert_eq!(blob, None);
    assert_eq!(timebase.last_emitted(), 12);
}

#[test]
fn retry_attempt_mints_a_distinct_next_ordinal() {
    let mut timebase = TimebaseGenerator::firmware_lease(12, 20).unwrap();
    let first = NodeChannel::<32, 8, 4>::announce_blob_if_due(
        &mut timebase,
        true,
        5_000,
        [1; ANNOUNCE_NONCE_LEN],
    )
    .unwrap()
    .expect("due announce");
    // This models `retry_announce`: it makes the node due again after a
    // rejected transmit, and the retry must not reuse the first stamp.
    let retry = NodeChannel::<32, 8, 4>::announce_blob_if_due(
        &mut timebase,
        true,
        5_005,
        [2; ANNOUNCE_NONCE_LEN],
    )
    .unwrap()
    .expect("due retry");

    assert_eq!(first.timebase(), 13);
    assert_eq!(retry.timebase(), 14);
    assert_ne!(first, retry);
}

#[test]
fn only_the_boards_own_announce_counts_as_its_announce() {
    use retinue::packet::{DestinationType, HeaderType, Propagation};
    let own = AddressHash::from_bytes([1; 16]);
    let announce = |destination| Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Announce,
        hops: 0,
        transport: None,
        destination,
        context: 0,
        payload: alloc::vec::Vec::new(),
    };
    assert!(is_own_announce(own, &announce(own)));
    assert!(!is_own_announce(
        own,
        &announce(AddressHash::from_bytes([2; 16]))
    ));
    let mut data = announce(own);
    data.packet_type = PacketType::Data;
    assert!(!is_own_announce(own, &data));
}
