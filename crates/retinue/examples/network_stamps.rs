//! Local-only stock network stamp qualification peer. Driven by oracle/interop_network_stamps.py.
use retinue::endpoint::{Endpoint, NetworkStampPolicy, StampDirectory};
use retinue::stamp::StampCost;
use retinue::{AddressHash, DestinationName, PrivateIdentity, RatchetPolicy, RatchetStore};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(text: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if !text.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| Ok(u8::from_str_radix(&text[i..i + 2], 16)?))
        .collect()
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(6..=7).contains(&args.len()) {
        return Err(
            "PORT emit|plain|default enforce|permissive DEST_COST_FP PR_COST_FP LR_COST_FP [STORAGE]"
                .into(),
        );
    }
    let endpoint = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x47; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let cost = |i: usize| -> Result<StampCost, std::num::ParseIntError> {
        Ok(StampCost::from_fp(args[i].parse()?))
    };
    let policy = if args[1] == "default" {
        // The stock 1.6.1 defaults; only enforcement is chosen.
        NetworkStampPolicy {
            enforce: args[2] == "enforce",
            ..Default::default()
        }
    } else {
        NetworkStampPolicy {
            stamp_destinations: args[1] == "emit",
            stamp_path_requests: args[1] == "emit",
            enforce: args[2] == "enforce",
            destination_cost: cost(3)?,
            destination_value: cost(3)?,
            path_cost: cost(4)?,
            path_value: cost(4)?,
            link_cost: cost(5)?,
            generation_timeout: Duration::from_secs(3),
            max_attempts: 10_000_000,
            ..Default::default()
        }
    };
    endpoint.set_network_stamp_policy(policy);
    if let Some(storage) = args.get(6) {
        endpoint.set_destination_stamp_store(StampDirectory::in_storage(storage.as_ref()));
    }
    let name = DestinationName::new("retinue", ["network-stamp"]);
    endpoint.set_destination_link_cost(&name, cost(5)?)?;
    let port: u16 = args[0].parse()?;
    let listener = endpoint.listen_tcp(([127, 0, 0, 1], port).into()).await?;
    println!("LISTENING {}", listener.port());
    println!("DEST {}", name.destination_hash(endpoint.identity()));
    endpoint.register_with_ratchets(
        name.clone(),
        b"network-stamp",
        RatchetStore::new(RatchetPolicy::default())?,
    )?;
    let observer = Arc::clone(&endpoint);
    tokio::spawn(async move {
        while let Ok(fact) = observer.next_announcement().await {
            let ratchet = observer
                .destination_ratchet(fact.destination)
                .map_or_else(|| "none".into(), |v| hex(&v));
            let cost = observer
                .destination_link_cost(fact.destination)
                .map_or_else(|| "none".into(), |v| v.to_fp().to_string());
            println!(
                "APP {} {} {ratchet} {cost}",
                fact.destination,
                hex(&fact.app_data)
            );
        }
    });
    let acceptor = Arc::clone(&endpoint);
    tokio::spawn(async move {
        while let Ok(mut stream) = acceptor.accept().await {
            println!("INBOUND");
            tokio::spawn(async move {
                let mut buf = [0; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    let _ = stream.write_all(&buf[..n]).await;
                }
            });
        }
    });
    let (tx, mut rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = rx.recv().await {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            ["ANNOUNCE", data] => endpoint.announce(&name, &unhex(data)?),
            ["PATH", target] => {
                let hash = AddressHash::from_slice(&unhex(target)?).ok_or("bad target")?;
                println!("PATH_REQUEST {}", endpoint.request_path(hash));
            }
            ["LINK", target] => {
                let hash = AddressHash::from_slice(&unhex(target)?).ok_or("bad target")?;
                if let Some(peer) = endpoint.resolve(hash) {
                    let e = Arc::clone(&endpoint);
                    tokio::spawn(async move {
                        match e.open(hash, peer).await {
                            Ok(_stream) => println!("ACTIVE {hash}"),
                            Err(err) => println!("FAIL {err}"),
                        }
                    });
                } else {
                    println!("FAIL unknown peer");
                }
            }
            ["QUIT"] => break,
            _ => println!("FAIL unknown command"),
        }
    }
    endpoint.close();
    Ok(())
}
