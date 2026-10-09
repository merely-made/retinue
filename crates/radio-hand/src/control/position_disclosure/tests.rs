use super::*;

const SECRET: [u8; 32] = [0x5A; 32];

fn hash(seed: u8) -> [u8; 16] {
    [seed; 16]
}
fn entry(seed: u8, tier: DisclosureTier) -> PositionAclEntry {
    PositionAclEntry {
        hash: hash(seed),
        tier,
    }
}

#[test]
fn round_trips_canonically_and_sorts_on_construction() {
    let record = PositionAclV1::<8>::new(
        7,
        AbsentPolicy::Broadcast,
        &[
            entry(0x30, DisclosureTier::Precise),
            entry(0x10, DisclosureTier::Coarse),
            entry(0x20, DisclosureTier::Off),
        ],
    )
    .unwrap();
    let hashes: heapless::Vec<u8, 8> = record.entries().map(|e| e.hash[0]).collect();
    assert_eq!(&hashes[..], &[0x10, 0x20, 0x30]);
    let mut buf = [0u8; 128];
    let n = record.encode(&mut buf).unwrap();
    assert_eq!(n, POSITION_ACL_HEADER_LEN + 3 * POSITION_ACL_ENTRY_LEN);
    let decoded = PositionAclV1::<8>::decode(&buf[..n]).unwrap();
    assert_eq!(decoded, record);
    let mut again = [0u8; 128];
    let m = decoded.encode(&mut again).unwrap();
    assert_eq!(&buf[..n], &again[..m], "encoding is canonical");
}

#[test]
fn decode_rejects_every_non_canonical_form() {
    let record = PositionAclV1::<8>::new(
        1,
        AbsentPolicy::Fixed(DisclosureTier::Coarse),
        &[
            entry(0x10, DisclosureTier::Precise),
            entry(0x20, DisclosureTier::Precise),
        ],
    )
    .unwrap();
    let mut buf = [0u8; 128];
    let n = record.encode(&mut buf).unwrap();

    assert_eq!(
        PositionAclV1::<8>::decode(&buf[..n - 1]),
        Err(PositionAclError::Length)
    );
    let mut v = buf;
    v[0] = 9;
    assert_eq!(
        PositionAclV1::<8>::decode(&v[..n]),
        Err(PositionAclError::UnsupportedVersion(9))
    );
    let mut a = buf;
    a[9] = 0x7E;
    assert_eq!(
        PositionAclV1::<8>::decode(&a[..n]),
        Err(PositionAclError::InvalidAbsentPolicy(0x7E))
    );
    let mut t = buf;
    t[POSITION_ACL_HEADER_LEN + POSITION_ACL_HASH_LEN] = 3;
    assert_eq!(
        PositionAclV1::<8>::decode(&t[..n]),
        Err(PositionAclError::InvalidTier(3))
    );
    let mut swapped = buf;
    swapped[POSITION_ACL_HEADER_LEN..POSITION_ACL_HEADER_LEN + 16].copy_from_slice(&hash(0x20));
    swapped[POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN
        ..POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN + 16]
        .copy_from_slice(&hash(0x10));
    assert_eq!(
        PositionAclV1::<8>::decode(&swapped[..n]),
        Err(PositionAclError::NonCanonicalOrder)
    );
    let mut dup = buf;
    dup[POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN
        ..POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN + 16]
        .copy_from_slice(&hash(0x10));
    assert_eq!(
        PositionAclV1::<8>::decode(&dup[..n]),
        Err(PositionAclError::NonCanonicalOrder)
    );
}

#[test]
fn duplicate_hashes_are_refused_at_construction() {
    let err = PositionAclV1::<8>::new(
        1,
        AbsentPolicy::Broadcast,
        &[
            entry(0x10, DisclosureTier::Off),
            entry(0x10, DisclosureTier::Precise),
        ],
    )
    .unwrap_err();
    assert_eq!(err, PositionAclError::NonCanonicalOrder);
}

#[test]
fn capacity_refuses_and_never_evicts() {
    let too_many: [PositionAclEntry; 3] = [
        entry(1, DisclosureTier::Precise),
        entry(2, DisclosureTier::Precise),
        entry(3, DisclosureTier::Precise),
    ];
    assert_eq!(
        PositionAclV1::<2>::new(1, AbsentPolicy::Broadcast, &too_many).unwrap_err(),
        PositionAclError::Capacity {
            offered: 3,
            limit: 2
        }
    );
    let big = PositionAclV1::<8>::new(1, AbsentPolicy::Broadcast, &too_many).unwrap();
    let mut buf = [0u8; 128];
    let n = big.encode(&mut buf).unwrap();
    assert_eq!(
        PositionAclV1::<2>::decode(&buf[..n]).unwrap_err(),
        PositionAclError::Capacity {
            offered: 3,
            limit: 2
        }
    );
}

#[test]
fn no_table_resolves_everything_to_broadcast_and_never_fails() {
    let table = BlindedPositionAcl::<8>::new();
    assert_eq!(table.grants(), 0);
    assert_eq!(table.accepted_sequence(), None);
    assert_eq!(table.resolve(&hash(0x10), &SECRET), Resolved::Broadcast);
}

#[test]
fn absent_identity_gets_broadcast_by_default_and_fixed_when_set() {
    let mut table = BlindedPositionAcl::<8>::new();
    let whitelist = PositionAclV1::<8>::new(
        1,
        AbsentPolicy::Broadcast,
        &[entry(0x10, DisclosureTier::Precise)],
    )
    .unwrap();
    table.apply(&whitelist, &SECRET).unwrap();
    assert_eq!(
        table.resolve(&hash(0x10), &SECRET),
        Resolved::Tier(DisclosureTier::Precise)
    );
    assert_eq!(table.resolve(&hash(0x99), &SECRET), Resolved::Broadcast);

    let blacklist = PositionAclV1::<8>::new(
        2,
        AbsentPolicy::Fixed(DisclosureTier::Precise),
        &[entry(0x10, DisclosureTier::Off)],
    )
    .unwrap();
    table.apply(&blacklist, &SECRET).unwrap();
    assert_eq!(
        table.resolve(&hash(0x10), &SECRET),
        Resolved::Tier(DisclosureTier::Off)
    );
    assert_eq!(
        table.resolve(&hash(0x99), &SECRET),
        Resolved::Tier(DisclosureTier::Precise)
    );
}

#[test]
fn stored_form_holds_no_plaintext_hash() {
    let mut table = BlindedPositionAcl::<8>::new();
    let record = PositionAclV1::<8>::new(
        1,
        AbsentPolicy::Broadcast,
        &[entry(0x42, DisclosureTier::Precise)],
    )
    .unwrap();
    table.apply(&record, &SECRET).unwrap();
    let (tag, _) = table.tags[0].unwrap();
    assert_ne!(tag, hash(0x42), "tag must not be the plaintext hash");
    assert_ne!(
        table.resolve(&hash(0x42), &[0x00; 32]),
        Resolved::Tier(DisclosureTier::Precise),
        "a different secret must not resolve the grant"
    );
}

#[test]
fn pd2_sequence_is_monotonic_and_replay_is_refused_and_counted() {
    let mut table = BlindedPositionAcl::<8>::new();
    let five = PositionAclV1::<8>::new(5, AbsentPolicy::Broadcast, &[]).unwrap();
    let four = PositionAclV1::<8>::new(4, AbsentPolicy::Broadcast, &[]).unwrap();
    table.apply(&five, &SECRET).unwrap();
    assert_eq!(
        table.apply(&five, &SECRET).unwrap_err(),
        PositionAclError::NotMonotonic {
            offered: 5,
            accepted: 5
        }
    );
    assert_eq!(
        table.apply(&four, &SECRET).unwrap_err(),
        PositionAclError::NotMonotonic {
            offered: 4,
            accepted: 5
        }
    );
    assert_eq!(table.refused(), 2);
    assert_eq!(table.accepted_sequence(), Some(5));
}

#[test]
fn pd2_revocation_holds_and_a_lockout_table_is_still_correctable() {
    // The owner's own hash is just another identity to this table; it governs
    // disclosure, never command authority, so a table that refuses everyone
    // (including the owner) is correctable by the next higher-sequence write.
    let owner = hash(0xAA);
    let kin = hash(0xBB);
    let mut table = BlindedPositionAcl::<8>::new();
    let grant = PositionAclV1::<8>::new(
        1,
        AbsentPolicy::Broadcast,
        &[
            PositionAclEntry {
                hash: owner,
                tier: DisclosureTier::Precise,
            },
            PositionAclEntry {
                hash: kin,
                tier: DisclosureTier::Precise,
            },
        ],
    )
    .unwrap();
    table.apply(&grant, &SECRET).unwrap();

    // Revoke kin. Kin must drop to Broadcast, and there is no method to restore it.
    let revoked = PositionAclV1::<8>::new(
        2,
        AbsentPolicy::Broadcast,
        &[PositionAclEntry {
            hash: owner,
            tier: DisclosureTier::Precise,
        }],
    )
    .unwrap();
    table.apply(&revoked, &SECRET).unwrap();
    assert_eq!(table.resolve(&kin, &SECRET), Resolved::Broadcast);
    assert!(table.apply(&grant, &SECRET).unwrap_err().is_not_monotonic());
    assert_eq!(
        table.resolve(&kin, &SECRET),
        Resolved::Broadcast,
        "replay did not re-grant"
    );

    // Lockout: refuse everyone, owner included.
    let lockout = PositionAclV1::<8>::new(
        3,
        AbsentPolicy::Fixed(DisclosureTier::Off),
        &[PositionAclEntry {
            hash: owner,
            tier: DisclosureTier::Off,
        }],
    )
    .unwrap();
    table.apply(&lockout, &SECRET).unwrap();
    assert_eq!(
        table.resolve(&owner, &SECRET),
        Resolved::Tier(DisclosureTier::Off)
    );

    // The owner's next command still lands: nothing here gated it.
    let corrected = PositionAclV1::<8>::new(
        4,
        AbsentPolicy::Broadcast,
        &[PositionAclEntry {
            hash: owner,
            tier: DisclosureTier::Precise,
        }],
    )
    .unwrap();
    table.apply(&corrected, &SECRET).unwrap();
    assert_eq!(
        table.resolve(&owner, &SECRET),
        Resolved::Tier(DisclosureTier::Precise)
    );
    assert_eq!(table.accepted_sequence(), Some(4));
}

impl PositionAclError {
    fn is_not_monotonic(self) -> bool {
        matches!(self, Self::NotMonotonic { .. })
    }
}
