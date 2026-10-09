use super::*;

/// One lost part no longer kills a transfer: the receiver's poll re-requests exactly
/// what is missing, and the sender serves it. This is the mechanism N5's first hardware
/// run proved was absent, when one dropped frame at SF11 stalled a five-part transfer
/// forever on a clean link.
#[test]
fn a_lost_part_is_re_requested_on_poll() {
    let (mut a, mut b, id) = linked();
    // Drain the boot announce, so later polls answer only for the transfer.
    let _ = b.poll(0, IFACE, Some(&blob([0; RAND_HASH_LEN])));
    let payload: Vec<u8> = (0..1_024u32).map(|i| (i.wrapping_mul(7)) as u8).collect();

    let started = a
        .publish(
            id,
            IFACE,
            &payload,
            [0xEE; 4],
            &[7; crate::token::IV_LEN],
            0,
        )
        .unwrap();

    // Deliver the advertisement, take b's request, serve it — but LOSE one part.
    let advertisement = sent(&started).unwrap();
    let request = sent(&b.ingest(IFACE, &advertisement, 0)).unwrap();
    let parts: Vec<Packet> = a
        .ingest(IFACE, &request, 0)
        .into_iter()
        .filter_map(|x| match x {
            Action::Send { packet, .. } => Some(packet),
            _ => None,
        })
        .collect();
    assert!(parts.len() >= 2, "the window carries several parts");
    let mut arrived = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index == 1 {
            continue; // the air ate it
        }
        arrived.extend(b.ingest(IFACE, part, 0));
    }
    assert!(
        !arrived.iter().any(|x| matches!(x, Action::Send { .. })),
        "with a part outstanding, b waits rather than re-requesting early"
    );
    assert!(b.transfer_active(id), "the transfer is stalled, not dead");

    // Before the retry interval: silence. At it: the re-request, unprompted.
    assert!(
        b.poll(
            RESOURCE_RETRY_INTERVAL - 1,
            IFACE,
            Some(&blob([0; RAND_HASH_LEN]))
        )
        .is_empty(),
        "no retry before its time"
    );
    let retry = sent(&b.poll(
        RESOURCE_RETRY_INTERVAL,
        IFACE,
        Some(&blob([0; RAND_HASH_LEN])),
    ))
    .expect("the poll re-requests the missing part");

    // The sender answers with the missing part, and the transfer completes.
    let served: Vec<Packet> = a
        .ingest(IFACE, &retry, 0)
        .into_iter()
        .filter_map(|x| match x {
            Action::Send { packet, .. } => Some(packet),
            _ => None,
        })
        .collect();
    let mut done = Vec::new();
    for part in &served {
        done.extend(b.ingest(IFACE, part, 0));
    }
    // Drain the remaining request/serve rounds if any, then check the payload landed.
    let mut to_a: Vec<Packet> = done
        .iter()
        .filter_map(|x| match x {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        })
        .collect();
    let mut received: Vec<Vec<u8>> = done
        .iter()
        .filter_map(|x| match x {
            Action::Resource { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect();
    for _ in 0..16 {
        if to_a.is_empty() {
            break;
        }
        let mut to_b = Vec::new();
        for packet in to_a.drain(..) {
            for action in a.ingest(IFACE, &packet, 0) {
                if let Action::Send { packet, .. } = action {
                    to_b.push(packet);
                }
            }
        }
        for packet in to_b {
            for action in b.ingest(IFACE, &packet, 0) {
                match action {
                    Action::Send { packet, .. } => to_a.push(packet),
                    Action::Resource { data, .. } => received.push(data),
                    _ => {}
                }
            }
        }
    }
    assert_eq!(received, vec![payload], "byte for byte, after the loss");
    assert!(
        !b.transfer_active(id),
        "and the receiver slot is free again"
    );
}

/// A lost advertisement is re-offered by the sender's poll, so a fetch whose first
/// offer the air ate still begins.
#[test]
fn a_lost_advertisement_is_re_offered_on_poll() {
    let (mut a, mut b, id) = linked();
    // Drain the boot announce, so the retry poll answers only for the transfer.
    let _ = a.poll(0, IFACE, Some(&blob([0; RAND_HASH_LEN])));
    let payload: Vec<u8> = (0..600u32).map(|i| i as u8).collect();

    // The advertisement from publish is LOST: b never hears it.
    let _ = a
        .publish(
            id,
            IFACE,
            &payload,
            [0xEF; 4],
            &[8; crate::token::IV_LEN],
            0,
        )
        .unwrap();
    assert!(a.transfer_active(id));

    // The idle link's keepalive goes out on the same poll; the offer is the advertisement.
    let again = a
        .poll(
            RESOURCE_RETRY_INTERVAL,
            IFACE,
            Some(&blob([0; RAND_HASH_LEN])),
        )
        .into_iter()
        .find_map(|action| match action {
            Action::Send { packet, .. } if packet.context == link::CTX_RESOURCE_ADV => Some(packet),
            _ => None,
        })
        .expect("the poll re-advertises the unanswered offer");
    let request = sent(&b.ingest(IFACE, &again, 0));
    assert!(request.is_some(), "and the re-offer starts the transfer");
}

/// Two sealed packets never share an IV, even across separate ingest calls. The
/// counter is node state; a fresh counter per call would replay the sequence.
#[test]
fn derived_ivs_never_repeat_across_calls() {
    let (mut a, mut b, id) = linked();
    let payload: Vec<u8> = (0..600u32).map(|i| i as u8).collect();

    let started = a
        .publish(
            id,
            IFACE,
            &payload,
            [0xAA; 4],
            &[9; crate::token::IV_LEN],
            0,
        )
        .unwrap();
    let advertisement = sent(&started).unwrap();
    let first = sent(&b.ingest(IFACE, &advertisement, 0)).expect("first request");
    // The same advertisement again: the receiver rebuilds the same logical request. If
    // IVs repeated, the sealed bytes would be identical.
    let second = sent(&b.ingest(IFACE, &advertisement, 0)).expect("second request");
    assert_ne!(
        first.payload, second.payload,
        "the same request sealed twice must differ, or the IV repeated"
    );
}

/// Losing the link discards the transfer riding on it.
///
/// Reassembly state without a link is memory held for a peer that is gone, which on a
/// board is exactly the leak worth preventing.
#[test]
fn closing_a_link_discards_its_transfer() {
    let (mut a, mut b, id) = linked();
    let payload: Vec<u8> = (0..3_000u32).map(|i| i as u8).collect();

    // Start a transfer and deliver only the advertisement, so b is mid-receive.
    let started = a
        .publish(
            id,
            IFACE,
            &payload,
            [0xAB; 4],
            &[5; crate::token::IV_LEN],
            0,
        )
        .unwrap();
    let advertisement = sent(&started).unwrap();
    b.ingest(IFACE, &advertisement, 0);
    assert!(b.transfer_active(id), "b is mid-transfer");

    // a closes the link.
    let close = a
        .links
        .iter()
        .find(|(l, _, _)| l.id() == id)
        .map(|(l, _, _)| l.close_packet(&[9; crate::token::IV_LEN]))
        .unwrap();
    let actions = b.ingest(IFACE, &close, 0);

    assert!(actions.iter().any(|x| matches!(x, Action::LinkDown { .. })));
    assert!(!b.transfer_active(id), "the transfer went with the link");
    assert_eq!(b.link_count(), 0);
}

#[test]
fn a_second_publish_on_a_busy_link_is_refused() {
    let (mut a, _b, id) = linked();
    let payload = vec![1_u8; 1_000];

    assert!(
        a.publish(id, IFACE, &payload, [1; 4], &[1; crate::token::IV_LEN], 0)
            .is_some(),
        "the first publish starts"
    );
    assert!(
        a.publish(id, IFACE, &payload, [2; 4], &[2; crate::token::IV_LEN], 0)
            .is_none(),
        "the second is refused while the first runs"
    );
}
