//! Retinue's side of `oracle/interop_udp.py`: the datagram script over an RNS UDPInterface.
//!
//! ```text
//! udp_interop LISTEN FORWARD [--ifac] [--reuse] [--request DEST]
//! ```
//!
//! LISTEN and FORWARD are `ip:port`. With `--request` it first asks once for a path to
//! the stock destination DEST (hex), for the broadcast echo check. On stdin closing it
//! prints its carrier counters and exits.

#[path = "datagram/script.rs"]
mod script;

use std::time::Duration;

use retinue::hash::AddressHash;
use retinue::iface::udp::UdpConfig;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut addr = |what: &str| -> Result<_, String> {
        let text = args
            .next()
            .ok_or(format!("usage: udp_interop LISTEN FORWARD; no {what}"))?;
        text.parse().map_err(|e| format!("{what} {text}: {e}"))
    };
    let mut cfg = UdpConfig {
        listen: Some(addr("LISTEN")?),
        forward: Some(addr("FORWARD")?),
        ..UdpConfig::default()
    };
    let (mut ifac, mut request) = (false, None);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--ifac" => ifac = true,
            "--reuse" => cfg.reuse_address = true,
            "--request" => {
                let hex = args.next().ok_or("--request DEST")?;
                let bytes = hex::decode(&hex).map_err(|e| format!("{e}"))?;
                request = Some(AddressHash::from_slice(&bytes).ok_or("DEST is 16 bytes")?);
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let endpoint = script::endpoint();
    let carrier = endpoint
        .attach_udp(cfg, script::ifac(ifac))
        .await
        .map_err(|e| format!("attach: {e}"))?;
    println!("ATTACHED {} {:?}", carrier.id, carrier.local);
    if let Some(dest) = request {
        endpoint.request_path(dest);
        println!("PATH_REQUESTED {dest}");
        let ep = endpoint.clone();
        tokio::spawn(async move {
            while ep.route_to(dest).is_none() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            println!("PATH_FOUND {dest}");
        });
    }
    script::serve(endpoint.clone()).await;
    let c = carrier.counters();
    println!(
        "COUNTERS rx={} tx={} own_echo={} oversize={}",
        c.rx, c.tx, c.own_echo, c.oversize
    );
    endpoint.shutdown(Duration::from_secs(2)).await;
    println!("DONE");
    Ok(())
}
