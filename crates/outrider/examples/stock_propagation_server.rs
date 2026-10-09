//! Production Outrider propagation server oracle for stock LXMF clients.
//!
//! Serves every client link concurrently until killed. Prints `STORE` whenever the store
//! changes and `LINK_CLOSED` as each link ends, so a gate can match the stock side's view
//! against the node's.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use outrider::{
    NodePolicy, PROPAGATION_METADATA_NAME, PropagationAnnounce, PropagationCosts, PropagationNode,
    PropagationStore, PropagationStoreLimits, announce_propagation, register_propagation,
    serve_fetch,
};
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use rmpv::Value;

const NODE_SEED: [u8; 64] = [0x70; 64];

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

fn load_store(
    path: Option<&Path>,
    limits: PropagationStoreLimits,
) -> Result<PropagationStore, Box<dyn std::error::Error>> {
    let snapshot = match path.map(std::fs::read) {
        Some(Ok(snapshot)) => snapshot,
        Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error.into());
        }
        _ => return Ok(PropagationStore::new(limits)),
    };
    let (store, receipt) = PropagationStore::restore(limits, &snapshot, now())?;
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

fn persist_store(path: Option<&Path>, store: &PropagationStore) -> std::io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let snapshot = store
        .encode_snapshot()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(&snapshot)?;
    file.sync_all()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let large = std::env::var("OUTRIDER_LARGE").is_ok_and(|value| value == "1");
    let store_path: Option<PathBuf> = std::env::var_os("OUTRIDER_STORE_PATH").map(PathBuf::from);
    let announce = PropagationAnnounce {
        legacy: false,
        unix_time: now() as u64,
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
    let mut limits = PropagationStoreLimits::default();
    if large {
        limits.max_message_bytes = 16 * 1024;
        limits.max_bytes = 64 * 1024;
    }
    let node = Arc::new(PropagationNode::new(
        load_store(store_path.as_deref(), limits)?,
        NodePolicy::from_announce(&announce),
    ));
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &NODE_SEED,
    )));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    endpoint.enable_routing();
    let destination = register_propagation(&endpoint, &announce)?;
    println!("LISTENING {}", address.port());
    println!("PROPAGATION_DESTINATION {destination}");
    tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move {
            loop {
                announce_propagation(&endpoint, &announce).expect("fixed announce encodes");
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
        }
    });
    // Report and persist each change of the store, whichever link made it.
    tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let mut last = None;
            loop {
                let (state, persisted) = {
                    let store = node.store();
                    let state = (store.len(), store.bytes());
                    let persisted =
                        (last != Some(state)).then(|| persist_store(store_path.as_deref(), &store));
                    (state, persisted)
                };
                if let Some(persisted) = persisted {
                    println!("STORE entries={} bytes={}", state.0, state.1);
                    if let Err(error) = persisted {
                        println!("STORE_PERSIST_FAILED {error}");
                    }
                    last = Some(state);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });

    loop {
        let accepted = endpoint.accept_resource().await?;
        let endpoint = Arc::clone(&endpoint);
        let node = Arc::clone(&node);
        tokio::spawn(async move {
            match serve_fetch(&endpoint, accepted, &node, now).await {
                Ok(served) => println!(
                    "LINK_CLOSED stored={} duplicates={} rejected={} offered={} served={} acknowledged={}",
                    served.stored.inserted,
                    served.stored.duplicates,
                    served.rejected,
                    served.offered.len(),
                    served.served.len(),
                    served.acknowledged
                ),
                Err(error) => println!("LINK_FAILED {error}"),
            }
        });
    }
}
