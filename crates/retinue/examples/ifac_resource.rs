//! Retinue half of the IFAC carrier gates, dialling a stock RNS `TCPServerInterface`.
//!
//! `ifac_resource MODE PORT SIZE`, where SIZE is `stream`, `serial`, or the stock
//! config's `ifac_size` in bits:
//!
//! - `resource` (`oracle/interop_ifac_resource.py`): accept RNS's link, receive its
//!   64 KiB Resource, prove its 431-byte link packet, then publish one back. With IFAC
//!   that packet, and at 64-byte codes every Resource part, is past the bare MTU.
//! - `announce SECS` (`oracle/interop_ifac_default.py`): announce for `SECS` seconds and
//!   report whether RNS's announce validated, to check the per-carrier default code size.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ReceivedPayload, ResourceSession, ResourceTransferConfig};
use retinue::hash::full_hash;
use retinue::identity::PrivateIdentity;
use retinue::ifac::{self, Ifac};

const NETWORK_NAME: &str = "retinue-ifac-carrier";
const PASSPHRASE: &str = "full-size-frames";
const RETINUE_SEED: [u8; 64] = [0x4e; 64];
/// The stock announcer in `announce` mode; the Python side derives the same identity.
const RNS_SEED: [u8; 64] = [0x5e; 64];
const RESOURCE_LEN: usize = 64 * 1024;
/// RNS's link MDU at MTU 500: a 499-byte logical packet.
const LINK_MDU: usize = 431;
const SEED: u32 = 0x1FAC;

/// The xorshift32 stream `oracle/library_gate.py` also generates: incompressible.
fn payload(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.max(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn access(size: &str) -> Result<Ifac, String> {
    let (name, pass) = (Some(NETWORK_NAME), Some(PASSPHRASE));
    let built = match size {
        "stream" => Ifac::for_stream(name, pass).map(Some),
        "serial" => Ifac::for_serial(name, pass).map(Some),
        bits => {
            let bits = bits.parse().map_err(|_| format!("bad size {bits}"))?;
            Ifac::from_config_bits(name, pass, Some(bits), ifac::STREAM_SIZE)
        }
    };
    built
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no IFAC credentials".into())
}

fn transfer(timeout: Duration) -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout,
        ..ResourceTransferConfig::default()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, port, size) = match args.as_slice() {
        [mode, port, size, ..] => (mode.as_str(), port.parse::<u16>()?, size.as_str()),
        _ => return Err("usage: ifac_resource MODE PORT SIZE [SECS]".into()),
    };
    let ifac = access(size)?;
    println!("IFAC_SIZE {}", ifac.size());

    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &RETINUE_SEED,
    )));
    endpoint
        .attach_tcp_client_with_ifac(([127, 0, 0, 1], port).into(), ifac)
        .await?;
    println!("ATTACHED");

    let run = match mode {
        "resource" => resource(&endpoint).await,
        "announce" => {
            let secs = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(15);
            announce(&endpoint, Duration::from_secs(secs)).await;
            Ok(())
        }
        other => Err(format!("unknown mode {other}")),
    };
    if let Err(error) = &run {
        println!("MODE_ERR {error}");
    }
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.shutdown(Duration::from_secs(3)),
    )
    .await;
    println!("DONE");
    run.map_err(Into::into)
}

/// Announce `retinue.ifac-default` until `hold` ends, reporting RNS's announce once.
async fn announce(endpoint: &Endpoint, hold: Duration) {
    let name = DestinationName::new("retinue", ["ifac-default"]);
    endpoint.register(name.clone(), b"ifac-default");
    let rns = DestinationName::new("retinue", ["ifac_stock"])
        .destination_hash(PrivateIdentity::from_secret_bytes(&RNS_SEED).public());
    let deadline = tokio::time::Instant::now() + hold;
    let mut seen = false;
    while tokio::time::Instant::now() < deadline {
        endpoint.announce(&name, b"ifac-default");
        let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.next_announcement()).await;
        if !seen && endpoint.resolve(rns).is_some() {
            seen = true;
            println!("RNS_ANNOUNCE_OK {rns}");
        }
    }
}

async fn resource(endpoint: &Arc<Endpoint>) -> Result<(), String> {
    let mut session = accept(endpoint).await?;
    session.set_config(transfer(Duration::from_secs(60)));

    let expected = payload(RESOURCE_LEN, SEED);
    match session.receive().await {
        Ok(ReceivedPayload::Resource(data)) => {
            println!("RESOURCE {} {}", data.len(), hex(&full_hash(&data)));
            println!(
                "{}",
                if data == expected {
                    "RESOURCE_OK"
                } else {
                    "RESOURCE_MISMATCH"
                }
            );
        }
        other => return Err(format!("expected a Resource: {other:?}")),
    }

    match session.receive().await {
        Ok(ReceivedPayload::Data(data)) => {
            let ok = data == payload(LINK_MDU, SEED + 1);
            println!("DATA {} {}", data.len(), if ok { "OK" } else { "MISMATCH" });
            session
                .prove_data()
                .map_err(|e| format!("prove_data: {e}"))?;
            println!("DATA_PROVED");
        }
        other => return Err(format!("expected a data packet: {other:?}")),
    }

    let back = payload(RESOURCE_LEN, SEED + 2);
    match session.publish(&back).await {
        Ok(()) => println!("PUBLISH_OK {}", back.len()),
        Err(e) => println!("PUBLISH_ERR {:?} {e}", e.kind()),
    }

    // Keep the link until RNS hangs up, so its callbacks finish before our close lands.
    session.set_config(transfer(Duration::from_secs(30)));
    match session.receive().await {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => println!("PEER_CLOSED"),
        Err(e) => println!("HOLD_END {:?} {e}", e.kind()),
        Ok(_) => println!("HOLD_UNEXPECTED_PAYLOAD"),
    }
    Ok(())
}

/// Announce `retinue.ifac-resource` until RNS opens a link to it.
async fn accept(endpoint: &Arc<Endpoint>) -> Result<ResourceSession, String> {
    let name = DestinationName::new("retinue", ["ifac-resource"]);
    endpoint.register_resource(name.clone(), b"ifac-resource");
    let announcer = Arc::clone(endpoint);
    let announcing = tokio::spawn(async move {
        loop {
            announcer.announce(&name, b"ifac-resource");
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    });
    let accepted = tokio::time::timeout(Duration::from_secs(30), endpoint.accept_resource()).await;
    announcing.abort();
    let session = accepted
        .map_err(|_| "no resource link".to_string())?
        .map_err(|e| format!("accept_resource: {e}"))?
        .session;
    println!("LINK {}", session.link_id());
    Ok(session)
}
