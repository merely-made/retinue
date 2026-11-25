//! A PipeInterface peer for `oracle/interop_pipe.py`.
//!
//! With no arguments it serves an endpoint over its own stdin and stdout, so it can be a stock
//! PipeInterface's `command`. With `spawn PROGRAM [ARGS...]` it attaches a pipe to that child
//! instead. Either way it announces `retinue.pipe-peer` every 3 s, proves every link packet,
//! and reports on stderr, since stdout may be the pipe: `DEST <hash>`, `LINK <id>`,
//! `DATA <len>`, and in spawn mode `IFACE <id>` and `ONLINE <id> <bool>` on each change.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{
    Endpoint, PIPE_HW_MTU, PipeConfig, ReceivedPayload, ResourceTransferConfig,
};
use retinue::identity::PrivateIdentity;
use retinue::iface::hdlc::{Deframer, frame};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ep = Arc::new(Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x91; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let argv: Vec<_> = std::env::args_os().skip(1).collect();
    if argv.first().is_some_and(|a| a == "spawn") {
        let id = ep.attach_pipe(PipeConfig::new(argv[1..].to_vec())).await?;
        eprintln!("IFACE {id}");
        let watched = Arc::clone(&ep);
        tokio::spawn(async move {
            let mut online = None;
            loop {
                let now = watched.interface_online(id);
                if online != Some(now) {
                    online = Some(now);
                    eprintln!("ONLINE {id} {now}");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
    } else {
        serve_stdio(&ep)?;
    }
    serve(ep).await;
    Ok(())
}

/// Carry an interface over this process's stdio with HDLC, as a stock pipe's child does.
/// Exits when stdin closes.
fn serve_stdio(ep: &Endpoint) -> std::io::Result<()> {
    let iface = ep.attach_interface_with_frame_limit(PIPE_HW_MTU)?;
    let (mut out, sink) = iface.split();
    std::thread::spawn(move || {
        let (mut deframer, mut buf) = (Deframer::with_limit(PIPE_HW_MTU), [0; 4096]);
        let mut stdin = std::io::stdin().lock();
        while let Ok(n @ 1..) = stdin.read(&mut buf) {
            for raw in deframer.push(&buf[..n]) {
                let _ = sink.deliver_frame(&raw);
            }
        }
        std::process::exit(0);
    });
    tokio::spawn(async move {
        let mut stdout = std::io::stdout();
        while let Some(pkt) = out.recv().await {
            let Ok(wire) = out.encode(&pkt) else { continue };
            if stdout
                .write_all(&frame(&wire))
                .and_then(|()| stdout.flush())
                .is_err()
            {
                std::process::exit(0);
            }
        }
    });
    Ok(())
}

/// Announce, report what is heard, and prove every packet on every link.
async fn serve(ep: Arc<Endpoint>) {
    let name = DestinationName::new("retinue", ["pipe-peer"]);
    ep.register_resource(name.clone(), b"pipe-peer");
    eprintln!("DEST {}", name.destination_hash(ep.identity()));
    let announcer = Arc::clone(&ep);
    tokio::spawn(async move {
        loop {
            announcer.announce(&name, b"pipe-peer");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
    while let Ok(accepted) = ep.accept_resource().await {
        let mut session = accepted.session;
        session.set_config(ResourceTransferConfig {
            timeout: Duration::from_secs(3600),
            ..ResourceTransferConfig::default()
        });
        eprintln!("LINK {}", session.link_id());
        tokio::spawn(async move {
            while let Ok(payload) = session.receive().await {
                if let ReceivedPayload::Data(data) = payload {
                    eprintln!("DATA {}", data.len());
                    let _ = session.prove_data();
                }
            }
        });
    }
}
