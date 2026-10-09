//! Ingress preservation: an accepted session reports the interface it arrived on.
//!
//! V3 of the 2026-07-24 low-power radio and managed-network plan: ingress is a transport
//! fact carried out of accept, surviving every accepted form and never crossed between
//! concurrent accepts.

use std::time::Duration;

use retinue::announce_admission::AnnounceIngressPolicy;
use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;

mod announce_burst;
mod flood;
mod interfaces;

/// Wait until `ep` can resolve `dest`, pumping announcements.
async fn await_resolve(ep: &Endpoint, dest: retinue::hash::AddressHash) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while ep.resolve(dest).is_none() && tokio::time::Instant::now() < deadline {
        let _ = tokio::time::timeout(Duration::from_millis(300), ep.next_announcement()).await;
    }
    assert!(ep.resolve(dest).is_some(), "peer should learn the dest");
}
