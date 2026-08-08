//! A saturated router must not detach a working interface.
//!
//! The endpoint's router queue is bounded at 1,024 packets. `try_send` into it fails both
//! when it is momentarily full and when the endpoint has been dropped, and for a while
//! `InterfaceSink::deliver` collapsed those into a single `false`. Every caller reads
//! `false` as "the endpoint is gone" and stops serving the interface, so a burst arriving
//! faster than the router drained it took the carrier down permanently, with nothing short
//! of a restart to bring it back. On a radio that carries other people's traffic, a burst is
//! ordinary weather.

use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::packet::Packet;

/// A packet the router will accept and then have to do something with.
fn probe(seq: u8) -> Packet {
    retinue::path::path_request(
        retinue::hash::AddressHash::from_bytes([seq; 16]),
        &[0x5A; 16],
    )
}

#[tokio::test]
async fn a_full_router_queue_drops_packets_without_detaching_the_interface() {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x11; 64]));
    let sink = endpoint.attach_interface().sink();

    // Far more than the 1,024-packet bound, delivered without yielding, so the router has no
    // chance to drain and the queue is certainly full well before the end.
    let burst = 4096;
    for seq in 0..burst {
        assert!(
            sink.deliver(probe(seq as u8)),
            "packet {seq} of {burst} reported the endpoint gone; it is not, the queue is \
             merely full, and treating that as terminal is what detached live radios",
        );
    }

    // The drops are real and counted rather than silent: this is a capacity fact that is
    // invisible from the wire and looks exactly like a peer that never transmitted.
    assert!(
        sink.dropped() > 0,
        "a 4096-packet burst into a 1024-deep queue must have dropped something",
    );

    // And the interface still works: once the router drains, delivery resumes on the same
    // sink. No reattach, no restart.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        sink.deliver(probe(0xFF)),
        "the interface must still deliver after the burst subsides",
    );
}

#[tokio::test]
async fn a_dropped_endpoint_does_stop_the_interface() {
    // The other half of the distinction: `false` must still mean something, and it means
    // this. Without it the fix above would just be "never stop", which would spin a carrier
    // against an endpoint that no longer exists.
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x22; 64]));
    let sink = endpoint.attach_interface().sink();
    assert!(sink.deliver(probe(1)), "alive while the endpoint lives");

    drop(endpoint);
    // Give the router task its chance to observe the drop and close the channel.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    assert!(
        !sink.deliver(probe(2)),
        "a gone endpoint must report gone, so a carrier stops instead of spinning",
    );
}
