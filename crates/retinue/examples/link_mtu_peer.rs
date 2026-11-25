//! Retinue half of `oracle/interop_link_mtu.py`: a link to or from stock RNS over TCP,
//! reporting its negotiated MTU and moving a 1 MiB Resource each way.
//!
//! `link_mtu_peer MODE PORT [CEILING]`:
//!
//! - `accept`: listen on 127.0.0.1:PORT, announce `retinue.link-mtu` until stock links,
//!   receive stock's Resource, then publish one back.
//! - `open`: dial stock's TCPServerInterface on PORT, link to the first `rns.link-mtu`
//!   announce, publish a Resource, then receive stock's.
//!
//! CEILING, if given, is passed to `set_link_mtu`: 500 gives the protocol-MTU baseline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ReceivedPayload, ResourceSession, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;

const RESOURCE_LEN: usize = 1024 * 1024;
const SEED: u32 = 0x4D7;
const WAIT: Duration = Duration::from_secs(60);

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, port) = match args.as_slice() {
        [mode, port, ..] => (mode.as_str(), port.parse::<u16>()?),
        _ => return Err("usage: link_mtu_peer accept|open PORT [CEILING]".into()),
    };
    let endpoint = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x6d; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    if let Some(ceiling) = args.get(2) {
        endpoint.set_link_mtu(ceiling.parse()?);
    }
    let session = match mode {
        "accept" => {
            endpoint.listen_tcp(([127, 0, 0, 1], port).into()).await?;
            println!("LISTENING");
            accept(&endpoint).await?
        }
        "open" => {
            endpoint
                .attach_tcp_client(([127, 0, 0, 1], port).into())
                .await?;
            println!("ATTACHED");
            open(&endpoint).await?
        }
        other => return Err(format!("unknown mode {other}").into()),
    };
    let run = exchange(session, mode == "open").await;
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

/// Announce until stock opens a link.
async fn accept(endpoint: &Arc<Endpoint>) -> Result<ResourceSession, String> {
    let name = DestinationName::new("retinue", ["link-mtu"]);
    endpoint.register_resource(name.clone(), b"link-mtu");
    let announcer = Arc::clone(endpoint);
    let announcing = tokio::spawn(async move {
        loop {
            announcer.announce(&name, b"link-mtu");
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    });
    let accepted = tokio::time::timeout(WAIT, endpoint.accept_resource()).await;
    announcing.abort();
    Ok(accepted
        .map_err(|_| "no link".to_string())?
        .map_err(|e| format!("accept_resource: {e}"))?
        .session)
}

/// Link to the first `rns.link-mtu` announce, over the route it arrived by.
async fn open(endpoint: &Endpoint) -> Result<ResourceSession, String> {
    let name = DestinationName::new("rns", ["link-mtu"]);
    let fact = tokio::time::timeout(WAIT, async {
        loop {
            match endpoint.next_announcement().await {
                Ok(fact) if fact.destination == name.destination_hash(&fact.identity) => {
                    return Ok(fact);
                }
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .map_err(|_| "no announce".to_string())?
    .map_err(|e| e.to_string())?;
    endpoint
        .open_resource(fact.destination, fact.identity)
        .await
        .map_err(|e| format!("open_resource: {e}"))
}

async fn exchange(mut session: ResourceSession, publish_first: bool) -> Result<(), String> {
    println!("LINK_MTU {}", session.mtu());
    session.set_config(ResourceTransferConfig {
        timeout: WAIT,
        ..ResourceTransferConfig::default()
    });
    if publish_first {
        publish(&mut session).await;
        receive(&mut session).await?;
    } else {
        receive(&mut session).await?;
        publish(&mut session).await;
    }
    // Hold the link until stock hangs up, so its callbacks finish first.
    match session.receive().await {
        Err(e) => println!("HOLD_END {:?}", e.kind()),
        Ok(_) => println!("HOLD_UNEXPECTED_PAYLOAD"),
    }
    Ok(())
}

async fn publish(session: &mut ResourceSession) {
    let started = Instant::now();
    match session.publish(&payload(RESOURCE_LEN, SEED + 1)).await {
        Ok(()) => println!(
            "PUBLISH_OK {RESOURCE_LEN} {:.3}",
            started.elapsed().as_secs_f64()
        ),
        Err(e) => println!("PUBLISH_ERR {:?} {e}", e.kind()),
    }
}

async fn receive(session: &mut ResourceSession) -> Result<(), String> {
    let started = Instant::now();
    match session.receive().await {
        Ok(ReceivedPayload::Resource(data)) => {
            let verdict = if data == payload(RESOURCE_LEN, SEED) {
                "OK"
            } else {
                "MISMATCH"
            };
            println!(
                "RESOURCE_{verdict} {} {:.3}",
                data.len(),
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        other => Err(format!("expected a Resource: {other:?}")),
    }
}
