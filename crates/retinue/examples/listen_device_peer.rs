//! Listeners bound by device and by host name, for `oracle/interop_listen_device.py`
//! (`TCPInterface.py` 469-551).
//!
//! `listen_device_peer [DEVICE]` (default `lo0`) listens on the device's IPv6 address, then on
//! `localhost` preferring IPv4, and prints `LISTENING device <addr>`, `LISTENING host <addr>`
//! and `DEST <hash>`. It announces `retinue.listen-device` every 3 s.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, ListenAddr, ListenPolicy};
use retinue::identity::PrivateIdentity;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = std::env::args().nth(1).unwrap_or_else(|| "lo0".into());
    let ep = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x1d; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let binds = [
        (
            "device",
            ListenAddr::Device {
                name: device,
                port: 0,
                prefer_ipv6: true,
            },
        ),
        (
            "host",
            ListenAddr::Host {
                name: "localhost".into(),
                port: 0,
                prefer_ipv6: false,
            },
        ),
    ];
    let mut listeners = Vec::new();
    for (label, addr) in binds {
        let listener = ep.listen_on(addr, ListenPolicy::default()).await?;
        println!("LISTENING {label} {}", listener.local_addr());
        listeners.push(listener);
    }
    let name = DestinationName::new("retinue", ["listen-device"]);
    ep.register(name.clone(), b"listen-device");
    println!("DEST {}", name.destination_hash(ep.identity()));
    loop {
        ep.announce(&name, b"listen-device");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
