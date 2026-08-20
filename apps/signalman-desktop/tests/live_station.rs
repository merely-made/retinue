//! S2's live bench leg, through the shipped live path.
//!
//! This test is inert unless `SIGNALMAN_STATION_PORT` and
//! `SIGNALMAN_PEER_PORT` both name attached boards running Retinue. It drives
//! the exact production route — `StationWorker` opens the lease-gated
//! `SitedStation`, polls the management snapshot getter, and the drained
//! events enter `DesktopState` through `apply_station_event` — while a second
//! real board announces over the air as an ordinary Postilion station. The
//! assertion is S2's own boundary: at least one real announce and one route
//! become visible in the one shared projection, with no multi-hop or delivery
//! claim.

use std::sync::Arc;
use std::time::{Duration, Instant};

use signalman_desktop::state::DesktopState;
use signalman_desktop::station::{StationEvent, StationSettings, StationWorker};

fn bench_ports() -> Option<(String, String)> {
    let station = std::env::var("SIGNALMAN_STATION_PORT").ok()?;
    let peer = std::env::var("SIGNALMAN_PEER_PORT").ok()?;
    if station.trim().is_empty() || peer.trim().is_empty() {
        return None;
    }
    Some((station, peer))
}

#[test]
fn live_station_shows_a_real_announce_and_route() {
    let Some((station_port, peer_port)) = bench_ports() else {
        eprintln!("live bench receipt skipped: SIGNALMAN_STATION_PORT/SIGNALMAN_PEER_PORT not set");
        return;
    };

    let data_root = tempfile::tempdir().expect("station data root");
    let worker = StationWorker::spawn(
        StationSettings {
            data_root: data_root.path().to_path_buf(),
            port: station_port.clone(),
            name: "S2 bench station".into(),
        },
        Arc::new(|| {}),
    );

    // The announcing peer is a plain Postilion station with a short-lived
    // random identity: real radio, real announce, no wallet claim.
    let peer = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("peer runtime");
        runtime.block_on(async {
            let mut secret = [0_u8; 64];
            getrandom::fill(&mut secret).expect("peer identity entropy");
            let identity = retinue::identity::PrivateIdentity::from_secret_bytes(&secret);
            let mut config =
                postilion::StationConfig::new(peer_port, "S2 bench peer", identity);
            config.announce_interval = Duration::from_secs(5);
            let station = postilion::Station::open(config).await.expect("peer opens");
            station.announce();
            // Hold the radio open long enough for several announces to land.
            tokio::time::sleep(Duration::from_secs(60)).await;
            drop(station);
        });
    });

    let catalog = signalman_desktop::default_catalog_path();
    let mut state = DesktopState::new(&catalog);
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut connected = false;
    let mut receipt: Option<(usize, Vec<String>)> = None;
    while Instant::now() < deadline {
        for event in worker.drain() {
            match &event {
                StationEvent::Failed { message } => panic!("live station failed: {message}"),
                StationEvent::Connected { name, port, .. } => {
                    connected = true;
                    println!("connected: {name} on {port}");
                }
                StationEvent::Snapshot { snapshot, .. } => {
                    println!(
                        "snapshot gen {:?}: {} routes, {} links, {} current announces, {} history",
                        snapshot.generation,
                        snapshot.routes.len(),
                        snapshot.links.len(),
                        snapshot.current_announces.len(),
                        snapshot.announce_history.len()
                    );
                }
            }
            state.apply_station_event(event);
        }
        let projection = state.network_projection();
        let vocabularies: Vec<String> = projection
            .relations
            .iter()
            .map(|relation| relation.fact.kind.vocabulary().to_owned())
            .collect();
        let heard = vocabularies
            .iter()
            .any(|kind| kind == "signalman:heard-announce");
        let routed = vocabularies.iter().any(|kind| kind == "signalman:route-via");
        if connected && projection.nodes.len() >= 2 && heard && routed {
            receipt = Some((projection.nodes.len(), vocabularies));
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let (nodes, vocabularies) =
        receipt.expect("no live announce and route arrived before the deadline");
    println!(
        "live receipt: station on {station_port}, {nodes} nodes, relations {vocabularies:?}"
    );
    drop(worker);
    let _ = peer.join();
}
