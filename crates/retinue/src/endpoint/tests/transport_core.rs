//! Behaviour shared by the two transport drivers.

use super::*;
use crate::packet::HeaderType;

#[tokio::test]
async fn type_two_route_misses_are_counted_once_and_filtered_before_lookup() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();
    let dest = AddressHash::from_bytes([0x7a; 16]);
    let mut data = single_packet(dest, vec![0x71; 48]);
    data.header_type = HeaderType::Type2;
    data.transport = Some(ep.identity().hash());
    route(&ep.shared, a.id(), data.clone());
    assert_eq!(ep.routing_counters().unroutable_packets, 1);
    route(&ep.shared, a.id(), data.clone());
    assert_eq!(ep.routing_counters().unroutable_packets, 1);
    assert_eq!(ep.routing_counters().filtered_packets, 1);
    ep.shared.learn_path(dest, b.id(), 0, None);
    route(&ep.shared, a.id(), data);
    assert_eq!(ep.routing_counters().unroutable_packets, 1);
    assert_eq!(ep.routing_counters().filtered_packets, 2);
    assert_eq!(ep.routing_counters().forwarded_packets, 0);
    assert!(a.outbound.queues.pop().is_none());
    assert!(b.outbound.queues.pop().is_none());
}

#[tokio::test]
async fn return_paths_are_transactional_with_output() {
    for state in ["empty", "full", "replace", "expired"] {
        for refusal in ["accepted", "full", "offline", "size", "egress", "outgoing"] {
            for via in [None, Some(AddressHash::from_bytes([0x7b; 16]))] {
                let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
                let a = ep.attach_interface();
                let b = ep.attach_interface();
                ep.enable_routing();
                let dest = AddressHash::from_bytes([0x7c; 16]);
                ep.shared.learn_path(dest, b.id(), 0, via);
                let learned = Instant::now() - Duration::from_secs(10);
                ep.shared
                    .path_table
                    .lock()
                    .unwrap()
                    .get_mut(&dest)
                    .unwrap()
                    .learned = learned;
                let mut data = single_packet(dest, vec![0x71; 48]);
                data.header_type = HeaderType::Type2;
                data.transport = Some(ep.identity().hash());
                if refusal == "size" {
                    data.payload = vec![0; 600];
                }
                let hash = data.hash();
                if state != "empty" {
                    for byte in 0..REVERSE_TABLE_CAPACITY as u8 {
                        let key = if state == "replace" && byte == 1 {
                            hash
                        } else {
                            AddressHash::from_bytes([byte; 16])
                        };
                        ep.shared.remember_reverse(key, 7, 8);
                        let age = if state == "expired" {
                            REVERSE_TIMEOUT + Duration::from_secs(u64::from(byte) + 1)
                        } else {
                            Duration::from_secs(u64::from(byte) + 1)
                        };
                        ep.shared
                            .reverse_table
                            .lock()
                            .unwrap()
                            .get_mut(&key)
                            .unwrap()
                            .at = Instant::now() - age;
                    }
                }
                let before = ep.shared.reverse_table.lock().unwrap().clone();
                match refusal {
                    "full" => {
                        while ep.shared.try_send_on_class(
                            b.id(),
                            data.clone(),
                            TrafficClass::Transit,
                        ) {}
                    }
                    "offline" => ep.shared.interfaces.lock().unwrap()[1]
                        .online
                        .store(false, Ordering::Release),
                    "egress" => {
                        ep.shared.routing.lock().unwrap().allowed_egress =
                            super::super::routing::InterfaceSelector::None
                    }
                    "outgoing" => {
                        assert!(ep.set_interface_outgoing(b.id(), false));
                    }
                    _ => {}
                }
                // Queue setup can address packets too; measure only the carried send.
                if let Some(entry) = ep.shared.path_table.lock().unwrap().get_mut(&dest) {
                    entry.learned = learned;
                    entry.used = None;
                }
                let generation = ep.shared.diagnostic_generation.load(Ordering::Acquire);
                route(&ep.shared, a.id(), data);
                let accepted = refusal == "accepted";
                let after = ep.shared.reverse_table.lock().unwrap();
                if accepted {
                    assert!(after.contains_key(&hash));
                    assert_eq!(
                        after.len(),
                        if matches!(state, "empty" | "expired") {
                            1
                        } else {
                            REVERSE_TABLE_CAPACITY
                        }
                    );
                } else {
                    assert_eq!(
                        *after, before,
                        "{state}/{refusal}: refused send restores the table"
                    );
                }
                let entry = ep.shared.path_table.lock().unwrap()[&dest];
                assert_eq!(
                    entry.used.is_some(),
                    accepted,
                    "{state}/{refusal}: only accepted output marks the route used"
                );
                assert_eq!(
                    entry.learned, learned,
                    "{state}/{refusal}: and moves nothing else"
                );
                assert_eq!(ep.routing_counters().forwarded_packets, u64::from(accepted));
                assert_eq!(
                    ep.shared.diagnostic_generation.load(Ordering::Acquire) - generation,
                    u64::from(accepted),
                    "{state}/{refusal}: route marked exactly once on acceptance"
                );
            }
        }
    }
}

#[tokio::test]
async fn transit_precedes_reverse_proofs_and_handles_once() {
    for bridged in [false, true] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        let c = ep.attach_interface();
        ep.enable_routing();
        let id = AddressHash::from_bytes([0x72; 16]);
        ep.shared.remember_reverse(id, c.id(), a.id());
        let mut proof = single_packet(id, vec![0x71; 48]);
        proof.packet_type = PacketType::Proof;
        if bridged {
            ep.shared.link_transport.lock().unwrap().insert(
                id,
                LinkBridge {
                    from: a.id(),
                    out: b.id(),
                    destination: id,
                    seen: Instant::now(),
                    proof_deadline: None,
                },
            );
        } else {
            proof.header_type = HeaderType::Type2;
            proof.transport = Some(ep.identity().hash());
        }
        route(&ep.shared, a.id(), proof);
        assert!(ep.shared.reverse_table.lock().unwrap().contains_key(&id));
        assert!(a.outbound.queues.pop().is_none());
        assert!(c.outbound.queues.pop().is_none());
        assert_eq!(b.outbound.queues.pop().is_some(), bridged);
    }
}

#[tokio::test]
async fn a_local_type_two_link_request_bypasses_transit_policy() {
    for routing in [false, true] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        if routing {
            ep.enable_routing();
        }
        let name = DestinationName::new("retinue", ["local"]);
        let dest = name.destination_hash(ep.identity());
        ep.register(name, b"");
        while a.outbound.queues.pop().is_some() {
            a.outbound.queues.delivery_complete();
        }
        let (pending, mut request) = link::PendingLink::open(
            dest,
            *ep.identity(),
            &[0x73; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        request.header_type = HeaderType::Type2;
        request.transport = Some(ep.identity().hash());
        route(&ep.shared, a.id(), request);
        let proof = a.outbound.queues.pop().expect("local destination accepts");
        assert!(pending.prove(&proof).is_ok());
        assert!(ep.shared.link_transport.lock().unwrap().is_empty());
        assert_eq!(ep.routing_counters().forwarded_packets, 0);
        assert_eq!(ep.routing_counters().policy_rejected, 0);
    }
}

#[tokio::test]
async fn carried_packets_filter_duplicates_only_after_bridge_admission() {
    for context in 0..=u8::MAX {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        let c = ep.attach_interface();
        ep.enable_routing();
        let id = AddressHash::from_bytes([0x73; 16]);
        ep.shared.link_transport.lock().unwrap().insert(
            id,
            LinkBridge {
                from: a.id(),
                out: b.id(),
                destination: id,
                seen: Instant::now(),
                proof_deadline: Some(Instant::now() + Duration::from_secs(10)),
            },
        );
        let mut data = single_packet(id, vec![0x71; 48]);
        data.destination_type = DestinationType::Link;
        data.context = context;
        route(&ep.shared, c.id(), data.clone());
        route(&ep.shared, a.id(), data.clone());
        assert!(b.outbound.queues.pop().is_none());
        ep.shared
            .link_transport
            .lock()
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .proof_deadline = None;
        route(&ep.shared, a.id(), data.clone());
        assert!(b.outbound.queues.pop().is_some());
        b.outbound.queues.delivery_complete();
        route(&ep.shared, b.id(), data);
        let exempt = !crate::node::is_deduplicated_link_context(context);
        assert_eq!(
            a.outbound.queues.pop().is_some(),
            exempt,
            "context {context}"
        );
        assert_eq!(
            ep.routing_counters().forwarded_packets,
            1 + u64::from(exempt)
        );
    }
}

#[tokio::test]
async fn carried_links_refresh_only_on_accepted_send() {
    for refusal in [
        "accepted",
        "full",
        "hops",
        "size",
        "policy",
        "egress",
        "offline",
        "duplicate",
        "proof",
        "proof-full",
    ] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        let id = AddressHash::from_bytes([0x74; 16]);
        let proves = refusal.starts_with("proof");
        let before = Instant::now() - Duration::from_secs(1);
        ep.shared.link_transport.lock().unwrap().insert(
            id,
            LinkBridge {
                from: a.id(),
                out: b.id(),
                destination: id,
                seen: before,
                proof_deadline: proves.then(|| Instant::now() + Duration::from_secs(10)),
            },
        );
        let mut data = single_packet(id, vec![0x71; 48]);
        data.destination_type = DestinationType::Link;
        if proves {
            data.packet_type = PacketType::Proof;
            data.context = link::CTX_LRPROOF;
        }
        let (from, out) = if proves { (&b, &a) } else { (&a, &b) };
        match refusal {
            "full" | "proof-full" => {
                while ep
                    .shared
                    .try_send_on_class(out.id(), data.clone(), TrafficClass::Transit)
                {
                }
            }
            "hops" => data.hops = MAX_HOPS - 1,
            "size" => data.payload = vec![0; 600],
            "policy" => ep.shared.routing.lock().unwrap().forward_packets = false,
            "egress" => {
                ep.shared.routing.lock().unwrap().allowed_egress =
                    super::super::routing::InterfaceSelector::None
            }
            "offline" => ep
                .shared
                .interfaces
                .lock()
                .unwrap()
                .iter()
                .find(|i| i.id == out.id())
                .unwrap()
                .online
                .store(false, Ordering::Release),
            "duplicate" => assert!(ep.shared.packet_is_new(data.context, data.hash())),
            _ => {}
        }
        route(&ep.shared, from.id(), data);
        let accepted = matches!(refusal, "accepted" | "proof");
        let bridge = ep.shared.link_transport.lock().unwrap()[&id];
        assert_eq!(bridge.seen > before, accepted, "{refusal}");
        assert_eq!(
            bridge.proof_deadline.is_none(),
            !proves || accepted,
            "{refusal}"
        );
    }
}

#[tokio::test]
async fn forwarded_count_tracks_only_accepted_output() {
    for refusal in ["accepted", "full", "size", "offline", "hops"] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        ep.enable_routing();
        let mut data = single_packet(AddressHash::from_bytes([0x75; 16]), vec![0x71; 48]);
        match refusal {
            "full" => {
                while ep
                    .shared
                    .try_send_on_class(a.id(), data.clone(), TrafficClass::Transit)
                {
                }
            }
            "size" => data.payload = vec![0; 600],
            "offline" => ep.shared.interfaces.lock().unwrap()[0]
                .online
                .store(false, Ordering::Release),
            "hops" => data.hops = MAX_HOPS - 1,
            _ => {}
        }
        let policy = ep.shared.routing.lock().unwrap().clone();
        let accepted = super::super::transit::forward_on(&ep.shared, a.id(), data, &policy);
        assert_eq!(accepted, refusal == "accepted");
        assert_eq!(
            ep.routing_counters().forwarded_packets,
            u64::from(accepted),
            "{refusal}"
        );
    }
}

/// A link request that is not sent leaves the bridge table as it was: nothing it would have
/// evicted is lost, and a retransmission does not replace its unproved bridge (D36).
#[tokio::test]
async fn a_refused_link_request_leaves_the_bridges_as_they_were() {
    for retransmission in [false, true] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        let dest = AddressHash::from_bytes([0x7e; 16]);
        ep.shared.learn_path(dest, b.id(), 0, None);
        let mut request = single_packet(dest, vec![0x7e; 32]);
        request.packet_type = PacketType::LinkRequest;
        request.header_type = HeaderType::Type2;
        request.transport = Some(ep.identity().hash());
        let link_id = request.hash();
        let now = Instant::now();
        for n in 0..LINK_TRANSPORT_CAPACITY as u8 {
            let key = if retransmission && n == 0 {
                link_id
            } else {
                AddressHash::from_bytes([n + 1; 16])
            };
            ep.shared.link_transport.lock().unwrap().insert(
                key,
                LinkBridge {
                    from: a.id(),
                    out: b.id(),
                    destination: dest,
                    seen: now - Duration::from_secs(10 - u64::from(n)),
                    proof_deadline: Some(now + Duration::from_secs(60)),
                },
            );
        }
        while ep
            .shared
            .try_send_on_class(b.id(), request.clone(), TrafficClass::Transit)
        {}
        let before = ep.shared.link_transport.lock().unwrap().clone();
        route(&ep.shared, a.id(), request);
        assert_eq!(
            *ep.shared.link_transport.lock().unwrap(),
            before,
            "retransmission {retransmission}"
        );
    }
}

#[tokio::test]
async fn short_link_requests_are_bridged_and_carried() {
    for len in [1, 32, 63] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        let dest = AddressHash::from_bytes([0x76; 16]);
        ep.shared.learn_path(dest, b.id(), 0, None);
        let mut request = single_packet(dest, vec![0x76; len]);
        request.packet_type = PacketType::LinkRequest;
        request.header_type = HeaderType::Type2;
        request.transport = Some(ep.identity().hash());
        let id = request.hash();
        route(&ep.shared, a.id(), request.clone());
        let carried = b.outbound.queues.pop().expect("short request is carried");
        let bridges = ep.shared.link_transport.lock().unwrap();
        assert_eq!(bridges.len(), 1, "short request has a bridge");
        let bridge = bridges
            .get(&id)
            .expect("id is derived from the short request");
        assert_eq!((bridge.from, bridge.out), (a.id(), b.id()));
        assert_eq!(bridge.destination, dest);
        assert!(bridge.proof_deadline.is_some());
        assert!(ep.shared.reverse_table.lock().unwrap().is_empty());
        assert_eq!(carried.payload, request.payload);
        assert!(
            link::accept(
                &carried,
                &PrivateIdentity::from_secret_bytes(&[0x77; 64]),
                &[0x78; 64],
                LinkTrailer {
                    mode: LinkMode::Aes256Cbc,
                    mtu: 500,
                }
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn with_carriage_off_a_packet_is_not_transit() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();
    ep.shared.routing.lock().unwrap().forward_packets = false;
    let id = AddressHash::from_bytes([0x7b; 16]);
    ep.shared.remember_reverse(id, b.id(), a.id());
    let reverse = ep.shared.reverse_table.lock().unwrap().clone();
    let link_id = AddressHash::from_bytes([0x7d; 16]);
    let bridge = LinkBridge {
        from: a.id(),
        out: b.id(),
        destination: link_id,
        seen: Instant::now() - Duration::from_secs(1),
        proof_deadline: None,
    };
    ep.shared
        .link_transport
        .lock()
        .unwrap()
        .insert(link_id, bridge);
    let dest = AddressHash::from_bytes([0x7c; 16]);
    ep.shared.learn_path(dest, b.id(), 0, None);

    let mut proof = single_packet(id, vec![0x71; 48]);
    proof.packet_type = PacketType::Proof;
    let mut data = single_packet(dest, vec![0x71; 48]);
    data.header_type = HeaderType::Type2;
    data.transport = Some(ep.identity().hash());
    let mut carried = single_packet(link_id, vec![0x71; 48]);
    carried.destination_type = DestinationType::Link;
    for packet in [proof, data, carried] {
        route(&ep.shared, a.id(), packet);
    }
    assert_eq!(
        *ep.shared.reverse_table.lock().unwrap(),
        reverse,
        "a proof's return path is kept, nor a new one recorded"
    );
    assert_eq!(ep.shared.link_transport.lock().unwrap()[&link_id], bridge);
    assert!(a.outbound.queues.pop().is_none());
    assert!(b.outbound.queues.pop().is_none());
    let counters = ep.routing_counters();
    assert_eq!(counters.forwarded_packets, 0);
    // The desktop still counts each packet it would have carried, once (D35).
    assert_eq!(counters.policy_rejected, 3);
}

#[tokio::test]
async fn reverse_proofs_keep_header_and_obey_hop_ceiling() {
    for hops in [2, 3] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x71; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        ep.shared.routing.lock().unwrap().max_hops = 4;
        let id = AddressHash::from_bytes([0x79; 16]);
        ep.shared.remember_reverse(id, a.id(), b.id());
        // A matching path must not stamp a reverse proof with its transport id.
        ep.shared
            .learn_path(id, a.id(), 0, Some(AddressHash::from_bytes([0x7A; 16])));
        let mut proof = single_packet(id, vec![0x71; 48]);
        proof.packet_type = PacketType::Proof;
        proof.hops = hops;
        route(&ep.shared, b.id(), proof.clone());
        assert!(ep.shared.reverse_table.lock().unwrap().is_empty());
        if hops == 2 {
            let mut expected = proof;
            expected.hops += 1;
            assert_eq!(a.outbound.queues.pop().unwrap().encode(), expected.encode());
        } else {
            assert!(a.outbound.queues.pop().is_none());
            assert_eq!(ep.routing_counters().hop_limit_dropped, 1);
        }
    }
}
