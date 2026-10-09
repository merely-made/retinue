use super::*;
use crate::stamp::find_streamed;

const MESSAGE_ID: [u8; 32] = [2; 32];

#[test]
fn a_ticket_stamp_is_the_truncated_hash_of_ticket_and_message_id() {
    // hashlib.sha256(bytes([1]*16) + bytes([2]*32)).digest()[:16], RNS's truncated_hash.
    assert_eq!(
        hex::encode(ticket_stamp(&[1; 16], &MESSAGE_ID)),
        "ac2bca3db969f4464f7b2759d64430ea"
    );
}

#[test]
fn a_matching_ticket_is_tried_before_proof_of_work() {
    let stamp = ticket_stamp(&[1; 16], &MESSAGE_ID);
    let tickets = [[9; 16], [1; 16]];
    assert_eq!(
        check_stamp(&MESSAGE_ID, Some(&stamp), 255, &tickets),
        Ok(StampOutcome::Ticket)
    );
    assert_eq!(StampOutcome::Ticket.value(), COST_TICKET);
    assert_eq!(
        check_stamp(&MESSAGE_ID, Some(&stamp), 8, &[[9; 16]]),
        Err(StampFault::Invalid)
    );
    assert_eq!(
        check_stamp(&MESSAGE_ID, None, 8, &tickets),
        Err(StampFault::Missing)
    );
}

#[test]
fn without_a_ticket_the_stamp_falls_back_to_proof_of_work() {
    let (work, value) =
        find_streamed(&MESSAGE_ID, MESSAGE_WORKBLOCK_ROUNDS, 6, [0; 32], 1 << 16).unwrap();
    assert_eq!(
        check_stamp(&MESSAGE_ID, Some(&work), 6, &[[1; 16]]),
        Ok(StampOutcome::Work(value))
    );
    assert_eq!(
        check_stamp(&MESSAGE_ID, Some(&work), (value + 1) as u8, &[]),
        Err(StampFault::Invalid)
    );
}

#[test]
fn the_field_value_reads_the_way_stock_writes_it_and_more() {
    let ticket = Ticket {
        expires: 1_760_000_000.25,
        ticket: [5; 16],
    };
    assert_eq!(Ticket::decode(&ticket.encode()), Some(ticket));

    let mut bin = vec![0xc4, 16];
    bin.extend_from_slice(&[5; 16]);
    let with = |head: &[u8], tail: &[u8]| [head, &bin, tail].concat();
    let integer = Ticket::decode(&with(&[0x92, 0xce, 0x68, 0xe0, 0x00, 0x00], &[]));
    assert_eq!(integer.map(|t| t.expires), Some(1_759_510_528.0));
    let trailing = Ticket::decode(&with(&[0x93, 0x01], &[0xc0]));
    assert_eq!(trailing.map(|t| t.expires), Some(1.0));

    assert_eq!(Ticket::decode(&with(&[0x91], &[])), None);
    assert_eq!(Ticket::decode(&[0x92, 0x01, 0xc4, 15]), None);
    assert_eq!(Ticket::decode(&[0x92, 0x01, 0xb0]), None);
}

#[test]
fn a_ticket_goes_into_a_raw_field_map_where_a_dictionary_insert_would() {
    let ticket = Ticket {
        expires: 3.5,
        ticket: [6; 16],
    };
    let empty = ticket.insert_into(&[0x80]).unwrap();
    assert_eq!(empty[0], 0x81);
    assert_eq!(Ticket::from_fields(&empty), Some(ticket));

    // {7: b"meta"} gains the ticket after its own entry.
    let meta = [0x81, 0x07, 0xc4, 4, b'm', b'e', b't', b'a'];
    let appended = ticket.insert_into(&meta).unwrap();
    assert_eq!(appended[0], 0x82);
    assert_eq!(&appended[1..8], &meta[1..]);
    assert_eq!(Ticket::from_fields(&appended), Some(ticket));

    // A second ticket replaces the first where it stood.
    let newer = Ticket {
        expires: 9.0,
        ticket: [8; 16],
    };
    let replaced = newer.insert_into(&appended).unwrap();
    assert_eq!(replaced.len(), appended.len());
    assert_eq!(Ticket::from_fields(&replaced), Some(newer));

    assert_eq!(Ticket::from_fields(&meta), None);
    assert!(ticket.insert_into(&[0x81, 0x07]).is_err());
    assert!(ticket.insert_into(&[0x90]).is_err());
}

#[cfg(feature = "std")]
mod book {
    use retinue::hash::AddressHash;
    use retinue::identity::PrivateIdentity;
    use rmpv::Value;

    use super::super::*;
    use crate::announce::delivery_destination;
    use crate::codec::{LxmfPayload, decode, prepare};

    const NOW: f64 = 1_760_000_000.0;
    const DAY: f64 = 86_400.0;
    const PEER: AddressHash = AddressHash::from_bytes([4; 16]);

    #[test]
    fn the_raw_and_decoded_field_writers_agree_byte_for_byte() {
        let ticket = Ticket {
            expires: NOW,
            ticket: [3; 16],
        };
        let mut fields = Value::Map(vec![(Value::from(7), Value::Binary(b"x".to_vec()))]);
        set_ticket_field(&mut fields, &ticket);
        let mut written = Vec::new();
        rmpv::encode::write_value(&mut written, &fields).unwrap();
        assert_eq!(
            ticket.insert_into(&[0x81, 0x07, 0xc4, 1, b'x']).unwrap(),
            written
        );
        assert_eq!(ticket_field(&fields), Some(ticket));
    }

    #[test]
    fn issuance_reuses_renews_and_waits_out_the_interval() {
        let mut book = TicketBook::new();
        let first = book.issue(PEER, NOW, [1; 16]).unwrap();
        assert_eq!(first.expires, NOW + 21.0 * DAY);
        // More than 14 days left: the same ticket, not the fresh bytes.
        assert_eq!(book.issue(PEER, NOW + 6.0 * DAY, [2; 16]), Some(first));
        // Less than 14 days left: a new one.
        let renewed = book.issue(PEER, NOW + 8.0 * DAY, [2; 16]).unwrap();
        assert_eq!(renewed.ticket, [2; 16]);

        book.delivered(PEER, NOW + 8.0 * DAY);
        assert_eq!(book.issue(PEER, NOW + 8.5 * DAY, [3; 16]), None);
        assert!(book.issue(PEER, NOW + 9.0 * DAY, [3; 16]).is_some());

        // Both issued tickets validate until they expire; grace only delays cleanup.
        assert_eq!(book.inbound(&PEER, NOW + 20.0 * DAY).len(), 2);
        assert_eq!(book.inbound(&PEER, NOW + 22.0 * DAY), vec![[2; 16]]);
        book.clean(NOW + 25.0 * DAY);
        assert_eq!(book.inbound.get(&PEER).map(Vec::len), Some(2));
        book.clean(NOW + 26.5 * DAY);
        assert_eq!(book.inbound.get(&PEER).map(Vec::len), Some(1));
        assert!(book.issued.is_empty());
    }

    fn signed(sender: &PrivateIdentity, to: AddressHash, payload: &LxmfPayload) -> Vec<u8> {
        let source = delivery_destination(sender.public());
        let prepared = prepare(*to.as_bytes(), *source.as_bytes(), payload).unwrap();
        let signature = sender.sign(prepared.signing_bytes());
        prepared.finish(signature)
    }

    #[test]
    fn tickets_are_learned_only_from_messages_their_source_signed() {
        let peer = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
        let me = PrivateIdentity::from_secret_bytes(&[0x32; 64]);
        let mine = delivery_destination(me.public());
        let theirs = delivery_destination(peer.public());

        let mut peer_book = TicketBook::new();
        let mut payload = LxmfPayload::text(NOW, b"", b"hello");
        let given = peer_book.include(&mut payload, mine, NOW, [7; 16]).unwrap();
        let mut packed = signed(&peer, mine, &payload);

        let mut book = TicketBook::new();
        let message = decode(&packed).unwrap();
        assert_eq!(book.learn(&message, me.public(), NOW), None, "wrong source");
        assert_eq!(book.learn(&message, peer.public(), NOW + 22.0 * DAY), None);
        let last = packed.len() - 1;
        packed[last] ^= 1;
        let forged = decode(&packed).unwrap();
        assert_eq!(
            book.learn(&forged, peer.public(), NOW),
            None,
            "bad signature"
        );
        assert_eq!(book.outbound(&theirs, NOW), None);

        assert_eq!(book.learn(&message, peer.public(), NOW), Some(given));
        assert_eq!(book.outbound(&theirs, NOW), Some(given));
        assert_eq!(book.outbound(&theirs, given.expires), None);

        // The reply carries the ticket stamp, which the peer's book accepts at any cost.
        let mut reply = LxmfPayload::text(NOW + 1.0, b"", b"reply");
        assert!(book.stamp(&mut reply, theirs, me.public(), NOW).unwrap());
        let received = decode(&signed(&me, theirs, &reply)).unwrap();
        assert_eq!(
            check_stamp(
                &received.message_id,
                received.payload.stamp.as_deref(),
                255,
                &peer_book.inbound(&mine, NOW),
            ),
            Ok(StampOutcome::Ticket)
        );
        assert!(
            !TicketBook::new()
                .stamp(&mut reply, theirs, me.public(), NOW)
                .unwrap()
        );
    }

    #[test]
    fn the_book_survives_a_snapshot() {
        let mut book = TicketBook::new();
        book.issue(PEER, NOW, [1; 16]);
        book.issue(PEER, NOW + 8.0 * DAY, [2; 16]);
        book.delivered(PEER, NOW);
        book.outbound.insert(
            AddressHash::from_bytes([5; 16]),
            Ticket {
                expires: NOW,
                ticket: [6; 16],
            },
        );
        let snapshot = book.encode_snapshot().unwrap();
        assert_eq!(TicketBook::restore(&snapshot).unwrap(), book);

        assert!(TicketBook::restore(&snapshot[..snapshot.len() - 1]).is_err());
        let mut future = snapshot.clone();
        future[1] = 2;
        assert!(matches!(
            TicketBook::restore(&future),
            Err(TicketBookError::UnsupportedVersion(2))
        ));
    }
}
