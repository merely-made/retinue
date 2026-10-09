//! Retinue's side of `oracle/interop_auto_feth.py`: the datagram script over AutoInterface.
//!
//! ```text
//! auto_peer DEVICE[,DEVICE...] [--ifac] [--no-multicast]
//! ```
//!
//! Prints `ADOPTED` per interface and `PEER`/`GONE` as peers come and go. On stdin closing
//! it prints its counters and exits; killing it is the peer-timeout check.

#[path = "datagram/script.rs"]
mod script;

use std::time::Duration;

use retinue::iface::auto::AutoConfig;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let devices = args
        .next()
        .ok_or("usage: auto_peer DEVICE[,DEVICE...] [flags]")?;
    let (mut ifac, mut multicast_tx) = (false, true);
    for flag in args {
        match flag.as_str() {
            "--ifac" => ifac = true,
            "--no-multicast" => multicast_tx = false,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let cfg = AutoConfig {
        devices: devices.split(',').map(String::from).collect(),
        ifac: script::ifac(ifac),
        multicast_tx,
        ..AutoConfig::default()
    };
    let endpoint = script::endpoint();
    let handle = endpoint
        .attach_auto(cfg)
        .await
        .map_err(|e| format!("attach_auto: {e}"))?;
    for a in handle.status().adopted {
        println!("ADOPTED {} {} {}", a.name, a.index, a.link_local);
    }
    let handle = std::sync::Arc::new(handle);
    let watcher = std::sync::Arc::clone(&handle);
    tokio::spawn(async move {
        let mut known = Vec::new();
        loop {
            let now: Vec<_> = watcher
                .status()
                .peers
                .iter()
                .map(|p| (p.addr, p.index))
                .collect();
            for (addr, index) in now.iter().filter(|p| !known.contains(*p)) {
                println!("PEER {addr} {index}");
            }
            for (addr, index) in known.iter().filter(|p| !now.contains(*p)) {
                println!("GONE {addr} {index}");
            }
            known = now;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    script::serve(endpoint.clone()).await;
    let status = handle.status();
    let c = status.counters;
    let echoed = status.adopted.iter().all(|a| a.echoed);
    println!(
        "COUNTERS rx={} tx={} mif_duplicates={} oversize={} peers_refused={} echoed={echoed}",
        c.rx, c.tx, c.mif_duplicates, c.oversize, c.peers_refused
    );
    endpoint.shutdown(Duration::from_secs(2)).await;
    println!("DONE");
    Ok(())
}
