//! The Retinue half of the vanished-peer live gate (review #15).
//!
//! Driven by `oracle/interop_reliable_vanish.py`. Stock RNS links in and reads a long
//! Buffer stream from Retinue; the driver then kills the RNS process. Retinue's channel
//! must give up after RNS's five tries and close the link, so the stream fails with
//! `TimedOut` instead of retransmitting and waiting forever.

use std::time::{Duration, Instant};

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const IDENTITY_SEED: [u8; 64] = [0x38; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&IDENTITY_SEED));
    let addr = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    println!("LISTENING {}", addr.port());
    tokio::time::sleep(Duration::from_millis(250)).await;

    let name = DestinationName::new("retinue", ["vanish-oracle"]);
    endpoint.register_reliable(name.clone(), b"vanish-oracle");
    let link = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            endpoint.announce(&name, b"vanish-oracle");
            if let Ok(link) =
                tokio::time::timeout(Duration::from_millis(600), endpoint.accept_reliable()).await
            {
                return link;
            }
        }
    })
    .await??;
    let link_id = link.link_id();
    println!("LINK {link_id}");

    // Stream to the peer until the link fails under the writer.
    let (mut reader, mut writer) = tokio::io::split(link);
    let writer_task = tokio::spawn(async move {
        let chunk = [b'V'; 1024];
        let mut sent = 0usize;
        while writer.write_all(&chunk).await.is_ok() {
            sent += chunk.len();
        }
        sent
    });

    // The peer sends nothing, so the read ends only when the link does.
    let mut buf = [0u8; 64];
    let started = Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(120), reader.read(&mut buf)).await;
    match outcome {
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::TimedOut => {
            println!(
                "CLOSED TimedOut after {:.1}s",
                started.elapsed().as_secs_f64()
            );
        }
        other => return Err(format!("expected a TimedOut stream, got {other:?}").into()),
    }
    if endpoint.link_facts().iter().any(|fact| fact.id == link_id) {
        return Err("the failed link is still registered".into());
    }
    println!("LINK_GONE");
    let sent = tokio::time::timeout(Duration::from_secs(5), writer_task).await??;
    println!("WRITER_STOPPED {sent}");
    println!("DONE");
    Ok(())
}
