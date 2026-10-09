//! A routing retinue node with one TCP listener per mode, for `oracle/interop_listener_mode.py`
//! and `oracle/interop_egress_stall.py`.
//!
//! RETINUE_LISTENERS is a comma list of `full` and `access_point`. Prints, one per line:
//! `DEST <hash>`, `LISTENING <mode> <port>`, `SPAWNED <mode> <id>` per accepted connection,
//! and `GONE <id>` when an interface is forgotten. Commands on stdin: `ANNOUNCE`;
//! `FLOOD <count> <interval ms>`, our own announce repeated with bulky app data; and
//! `ANNOUNCER <port>`, a second endpoint that dials that port and announces, printing
//! `ANNOUNCER_DEST <hash>`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ListenPolicy};
use retinue::identity::PrivateIdentity;
use retinue::node::InterfaceMode;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let modes = std::env::var("RETINUE_LISTENERS").unwrap_or_else(|_| "full".into());
    let identity = PrivateIdentity::from_secret_bytes(&[0x4d; 64]);
    let ep = Arc::new(Endpoint::new(identity.clone()));
    ep.enable_routing();
    let name = DestinationName::new("retinue", ["listener"]);
    ep.register(name.clone(), b"listener");
    println!("DEST {}", name.destination_hash(identity.public()));

    let mut listeners = Vec::new();
    for label in modes.split(',') {
        let mode = match label {
            "access_point" => InterfaceMode::AccessPoint,
            _ => InterfaceMode::Full,
        };
        let (spawned, mut ids) = mpsc::channel(16);
        let policy = ListenPolicy {
            mode,
            spawned: Some(spawned),
            ..ListenPolicy::default()
        };
        let listener = ep
            .listen_tcp_with(([127, 0, 0, 1], 0).into(), policy)
            .await?;
        println!("LISTENING {label} {}", listener.local_addr().port());
        let label = label.to_string();
        tokio::spawn(async move {
            while let Some(id) = ids.recv().await {
                println!("SPAWNED {label} {id}");
            }
        });
        listeners.push(listener);
    }

    let watcher = Arc::clone(&ep);
    tokio::spawn(async move {
        let mut known = BTreeSet::new();
        loop {
            let now: BTreeSet<_> = watcher.interface_ids().into_iter().collect();
            for gone in known.difference(&now) {
                println!("GONE {gone}");
            }
            known = now;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let mut announcers = Vec::new();
    let mut lines = stdin_lines();
    while let Some(line) = lines.recv().await {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["ANNOUNCE"] => ep.announce(&name, b"listener"),
            ["FLOOD", count, interval] => {
                let (count, interval): (u32, u64) = (count.parse()?, interval.parse()?);
                let (flooder, name) = (Arc::clone(&ep), name.clone());
                tokio::spawn(async move {
                    for _ in 0..count {
                        flooder.announce(&name, &[0x55; 300]);
                        tokio::time::sleep(Duration::from_millis(interval)).await;
                    }
                    println!("FLOOD_DONE");
                });
            }
            ["ANNOUNCER", port] => {
                let id = PrivateIdentity::from_secret_bytes(&[0x7a; 64]);
                let announcer = Endpoint::new(id.clone());
                announcer
                    .attach_tcp_client(([127, 0, 0, 1], port.parse()?).into())
                    .await?;
                let relayed = DestinationName::new("retinue", ["relayed"]);
                println!("ANNOUNCER_DEST {}", relayed.destination_hash(id.public()));
                // At once: a fresh interface sends what is queued before its carrier starts.
                announcer.announce(&relayed, b"relayed");
                announcers.push(announcer);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Stdin lines, read on a thread: the crate's tokio has no stdin.
fn stdin_lines() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}
