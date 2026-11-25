//! A retinue transport that answers and searches path requests by interface mode, for the
//! discovery gate (`oracle/interop_path_discovery.py`).
//!
//! Arguments are `port:mode` peers to dial on 127.0.0.1, in order (modes: full,
//! point_to_point, access_point, roaming, boundary, gateway, internal). Prints
//! `IFACE <index> <id>` per peer, then `UP <transport id>`. Stdin commands:
//! `route <hex>` prints `ROUTE <hex> <iface> <hops>` or `ROUTE <hex> none`; `counters` prints
//! `COUNTERS <answered> <searched> <egress limited> <ingress limited>`.

use std::io::BufRead;

use retinue::endpoint::Endpoint;
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::node::InterfaceMode;

fn mode(name: &str) -> Result<InterfaceMode, String> {
    Ok(match name {
        "full" => InterfaceMode::Full,
        "point_to_point" => InterfaceMode::PointToPoint,
        "access_point" => InterfaceMode::AccessPoint,
        "roaming" => InterfaceMode::Roaming,
        "boundary" => InterfaceMode::Boundary,
        "gateway" => InterfaceMode::Gateway,
        "internal" => InterfaceMode::Internal,
        other => return Err(format!("unknown mode {other}")),
    })
}

fn hash(hex: &str) -> Option<AddressHash> {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    AddressHash::from_slice(&bytes)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ep = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0xD5; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    ep.enable_routing();
    for (index, peer) in std::env::args().skip(1).enumerate() {
        let (port, name) = peer.split_once(':').ok_or("expected port:mode")?;
        let id = ep
            .attach_tcp_client(([127, 0, 0, 1], port.parse()?).into())
            .await?;
        assert!(ep.set_interface_mode(id, mode(name)?));
        println!("IFACE {index} {id}");
    }
    println!("UP {}", ep.identity().hash());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    while let Some(line) = rx.recv().await {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["route", hex] => match hash(hex).and_then(|dest| ep.route_to(dest)) {
                Some((iface, hops)) => println!("ROUTE {hex} {iface} {hops}"),
                None => println!("ROUTE {hex} none"),
            },
            ["counters"] => {
                let c = ep.routing_counters();
                println!(
                    "COUNTERS {} {} {} {}",
                    c.path_requests_answered,
                    c.path_requests_searched,
                    c.path_requests_egress_limited,
                    c.path_requests_ingress_limited
                );
            }
            _ => {}
        }
    }
    Ok(())
}
