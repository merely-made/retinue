//! Retinue's side of `oracle/interop_auto_sniff.py`: hear stock AutoInterface tokens.
//!
//! ```text
//! auto_sniff DEVICE [--group G] [--scope S] [--count N] [--self-test]
//! ```
//!
//! Joins the discovery group on DEVICE beside stock (both sockets share the port) and
//! checks N tokens: each must come from the link-local `select` adopts for DEVICE and equal
//! `peering_token` over it. With `--self-test` it then runs a whole AutoInterface on DEVICE
//! under a private group and reports whether its own multicast came back.

use std::sync::Arc;
use std::time::Duration;

use retinue::auto::{Scope, discovery_group, peering_token};
use retinue::iface::auto::{self, AutoConfig, sockets};
use retinue::iface::netinfo;

/// An unused UDP port; the AutoInterface also takes the next one for reverse peering.
fn free_port() -> Result<u16, String> {
    let socket = std::net::UdpSocket::bind("[::]:0").map_err(|e| e.to_string())?;
    Ok(socket.local_addr().map_err(|e| e.to_string())?.port())
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let device = args.next().ok_or("usage: auto_sniff DEVICE [flags]")?;
    let mut cfg = AutoConfig {
        devices: vec![device.clone()],
        ..AutoConfig::default()
    };
    let (mut count, mut self_test) = (3, false);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--group" => cfg.group_id = value()?.into_bytes(),
            "--scope" => cfg.scope = Scope::from_config(&value()?).ok_or("unknown scope")?,
            "--count" => count = value()?.parse().map_err(|e| format!("{e}"))?,
            "--self-test" => self_test = true,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let host = netinfo::interfaces().map_err(|e| e.to_string())?;
    let adopted = auto::select(&host, &cfg);
    let ours = adopted
        .first()
        .ok_or(format!("{device} cannot be adopted"))?;
    let group = discovery_group(&cfg.group_id, cfg.scope, cfg.addr_type);
    println!(
        "ADOPTED {} {} {} GROUP {group}",
        ours.name, ours.index, ours.link_local
    );

    let socket = sockets::multicast(group, ours.index, cfg.discovery_port)
        .map_err(|e| format!("multicast socket: {e}"))?;
    let expected = peering_token(&cfg.group_id, &ours.link_local);
    let mut buf = [0u8; 1024];
    let mut matched = 0;
    while matched < count {
        let got = tokio::time::timeout(Duration::from_secs(10), socket.recv_from(&mut buf)).await;
        let Ok(Ok((n, std::net::SocketAddr::V6(src)))) = got else {
            println!("SNIFF_TIMEOUT {matched}");
            return Ok(());
        };
        let src = retinue::auto::descope(*src.ip());
        let ok = src == ours.link_local && buf[..n] == expected;
        println!("TOKEN {src} {n} {}", if ok { "MATCH" } else { "OTHER" });
        matched += usize::from(ok);
    }
    println!("SNIFF_OK {matched}");

    if self_test {
        let endpoint = Arc::new(retinue::endpoint::Endpoint::new(
            retinue::identity::PrivateIdentity::from_secret_bytes(&[0x5f; 64]),
        ));
        let private = AutoConfig {
            group_id: format!("retinue-sniff-{}", std::process::id()).into_bytes(),
            discovery_port: free_port()?,
            data_port: free_port()?,
            ..cfg
        };
        let handle = endpoint
            .attach_auto(private)
            .await
            .map_err(|e| format!("attach_auto: {e}"))?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let status = handle.status();
        let echoed = status.adopted.iter().all(|a| a.echoed && a.carrier_up);
        println!("SELF_TEST echoed={echoed} peers={}", status.peers.len());
    }
    Ok(())
}
