//! Production Outrider propagation server oracle for stock LXMF clients.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use outrider::{
    PROPAGATION_METADATA_NAME, PropagationAnnounce, PropagationCosts, PropagationStore,
    PropagationStoreLimits, announce_propagation, receive_submission, register_propagation,
    serve_fetch,
};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use rmpv::Value;

const NODE_SEED: [u8; 64] = [0x70; 64];

fn now() -> Result<f64, std::time::SystemTimeError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64())
}

fn load_store(
    path: Option<&Path>,
    limits: PropagationStoreLimits,
    at: f64,
) -> Result<PropagationStore, Box<dyn std::error::Error>> {
    let Some(path) = path else {
        return Ok(PropagationStore::new(limits));
    };
    match std::fs::read(path) {
        Ok(snapshot) => {
            let (store, receipt) = PropagationStore::restore(limits, &snapshot, at)?;
            println!(
                "STORE_RESTORED loaded={} duplicates={} rejected={} expired={} evicted={}",
                receipt.loaded,
                receipt.duplicates,
                receipt.rejected_too_large,
                receipt.expired,
                receipt.evicted
            );
            Ok(store)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(PropagationStore::new(limits))
        }
        Err(error) => Err(error.into()),
    }
}

fn persist_store(
    path: Option<&Path>,
    store: &PropagationStore,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = path else {
        return Ok(());
    };
    let snapshot = store.encode_snapshot()?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(&snapshot)?;
    file.sync_all()?;
    println!(
        "STORE_PERSISTED entries={} bytes={} snapshot_bytes={}",
        store.len(),
        store.bytes(),
        snapshot.len()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let large = std::env::var("OUTRIDER_LARGE").is_ok_and(|value| value == "1");
    let store_path = std::env::var_os("OUTRIDER_STORE_PATH").map(std::path::PathBuf::from);
    let mut limits = PropagationStoreLimits::default();
    if large {
        limits.max_message_bytes = 16 * 1024;
        limits.max_bytes = 64 * 1024;
    }
    let mut store = load_store(store_path.as_deref(), limits, now()?)?;
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &NODE_SEED,
    )));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    endpoint.enable_routing();
    let announce = PropagationAnnounce {
        legacy: false,
        unix_time: now()? as u64,
        active: true,
        transfer_limit_kib: 256,
        sync_limit_kib: 10_240,
        costs: PropagationCosts {
            propagation: 13,
            flexibility: 3,
            peering: 8,
        },
        metadata: vec![(
            Value::from(PROPAGATION_METADATA_NAME),
            Value::Binary(b"Outrider Propagation Server".to_vec()),
        )],
    };
    let destination = register_propagation(&endpoint, &announce)?;
    println!("LISTENING {}", address.port());
    println!("PROPAGATION_DESTINATION {destination}");
    let announcer = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        let announce = announce.clone();
        async move {
            loop {
                announce_propagation(&endpoint, &announce).expect("fixed announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });

    let accepted =
        tokio::time::timeout(Duration::from_secs(180), endpoint.accept_resource()).await??;
    let received = receive_submission(
        &endpoint,
        accepted,
        13,
        16 * 1024 * 1024,
        if large { 16 * 1024 } else { 4_096 },
    )
    .await?;
    let stored = store.ingest(&received.batch, now()?);
    persist_store(store_path.as_deref(), &store)?;
    println!(
        "SERVER_STORED inserted={} rejected={} entries={} bytes={}",
        stored.inserted,
        stored.rejected_too_large,
        store.len(),
        store.bytes()
    );

    let mut accepted =
        tokio::time::timeout(Duration::from_secs(180), endpoint.accept_resource()).await??;
    let served = serve_fetch(&endpoint, &mut accepted, &mut store, now()?).await?;
    println!(
        "SERVER_SERVED offered={} served={} acknowledged={}",
        served.offered.len(),
        served.served.len(),
        served.acknowledged
    );
    persist_store(store_path.as_deref(), &store)?;
    if large {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    drop(accepted);
    announcer.abort();
    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
