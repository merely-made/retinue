//! Host snapshots projected onto each radio's on-device UI.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use radio_face::{
    DetailPolicy, EventKind, EventSource, HostSnapshot, IfacState, NodeSummary, PeerPath,
    PeerSummary, Personality, Text, UiEvent, encode_snapshot,
};
use retinue::endpoint::Endpoint;
use retinue::identity::Identity;
use tulle::direct_phy_serial::DirectPhyUiControl;

pub(super) static STAGE_SECS: AtomicU64 = AtomicU64::new(0);

pub(super) struct View<'a> {
    endpoint: &'a Endpoint,
    identity: &'a Identity,
    node_name: &'a str,
    peer_name: Option<&'a str>,
    detail: DetailPolicy,
    ifac: IfacState,
    links: u8,
    admitted: u8,
    event_kind: EventKind,
    event_text: &'a str,
    started: Instant,
}

fn host_snapshot(view: &View<'_>) -> HostSnapshot {
    let identity_hash = *view.identity.hash().as_bytes();
    let destination = outrider::delivery_destination(view.identity);
    let mut address_tail = [0_u8; 8];
    address_tail.copy_from_slice(&destination.as_bytes()[8..]);
    let named = view.detail == DetailPolicy::Named;
    HostSnapshot {
        valid_for_secs: radio_face::MAX_VALIDITY_SECS,
        personality: Personality::Retinue,
        detail: view.detail,
        node: named.then_some(NodeSummary {
            name: Text::from_truncated(view.node_name),
            address_tail,
            fingerprint: identity_hash,
            role: Text::from_truncated("OUTRIDER"),
            uptime_secs: view.started.elapsed().as_secs().min(u64::from(u32::MAX)) as u32,
        }),
        link_count: view.links,
        admitted_links: view.admitted,
        queue_depth: view
            .endpoint
            .outbound_queue_depth()
            .min(usize::from(u16::MAX)) as u16,
        ifac: view.ifac,
        peers: [
            view.peer_name.map(|name| PeerSummary {
                name: Text::from_truncated(name),
                path: PeerPath::Direct,
                age_secs: 0,
            }),
            None,
            None,
        ],
        peer_overflow: 0,
        event: Some(UiEvent {
            source: EventSource::Host,
            kind: view.event_kind,
            text: Text::from_truncated(view.event_text),
        }),
    }
}

pub(super) async fn publish(
    control: &DirectPhyUiControl,
    view: &View<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = host_snapshot(view);
    let mut encoded = [0_u8; radio_face::MAX_SNAPSHOT_LEN];
    let len = encode_snapshot(&snapshot, &mut encoded).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("host snapshot did not encode: {error:?}"),
        )
    })?;
    control.publish(&encoded[..len]).await?;
    let stage_secs = STAGE_SECS.load(Ordering::Relaxed);
    if stage_secs > 0 {
        println!("ui stage: {} for {stage_secs}s", view.event_text);
        tokio::time::sleep(Duration::from_secs(stage_secs)).await;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn view<'a>(
    endpoint: &'a Endpoint,
    identity: &'a Identity,
    node_name: &'a str,
    peer_name: Option<&'a str>,
    started: Instant,
    detail: DetailPolicy,
    ifac: IfacState,
    links: u8,
    admitted: u8,
    event_kind: EventKind,
    event_text: &'a str,
) -> View<'a> {
    View {
        endpoint,
        identity,
        node_name,
        peer_name,
        detail,
        ifac,
        links,
        admitted,
        event_kind,
        event_text,
        started,
    }
}
