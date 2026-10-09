//! A retinue transport that dials stock TCP servers, one interface mode each, for the
//! interface-mode gate (`oracle/interop_interface_modes.py`).
//!
//! RETINUE_PEERS is a comma list of `port:mode` (modes: full, access_point, roaming,
//! boundary, gateway, internal, point_to_point). Prints `MODE_TRANSPORT_UP <own destination>`
//! once every peer is attached; each `announce` line on stdin then announces that destination.

use std::io::BufRead;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0xE3; 64]));
    ep.enable_routing();
    for peer in std::env::var("RETINUE_PEERS")?.split(',') {
        let (port, name) = peer.split_once(':').ok_or("expected port:mode")?;
        let id = ep
            .attach_tcp_client(([127, 0, 0, 1], port.parse()?).into())
            .await?;
        assert!(ep.set_interface_mode(id, mode(name)?));
    }
    let name = DestinationName::new("retinue", ["modes"]);
    println!("MODE_TRANSPORT_UP {}", name.destination_hash(ep.identity()));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let mut registered = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(3600), rx.recv()).await {
            Ok(Some(line)) if line.trim() == "announce" => {
                if registered {
                    ep.announce(&name, b"");
                } else {
                    ep.register(name.clone(), b"");
                    registered = true;
                }
                println!("ANNOUNCED");
            }
            Ok(None) => return Ok(()),
            _ => {}
        }
    }
}
