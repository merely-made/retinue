//! The Retinue half of the stock-RNS Channel/Buffer live gate.
//!
//! Driven by `oracle/interop_reliable_stream.py`. RNS initiates a real link and
//! writes through its Buffer; Endpoint reads through its reliable stream.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const IDENTITY_SEED: [u8; 64] = [0x37; 64];
const PREFIX: &[u8] = b"rns-compressed-eof:";
const REPLY_PREFIX: &[u8] = b"retinue-reliable-reply:";
const COMBINED_PREFIX: &[u8] = b"rns-combined-eof:";
const COMBINED_REPLY: &[u8] = b"retinue-combined-reply";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&IDENTITY_SEED));
    let addr = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    println!("LISTENING {}", addr.port());
    tokio::time::sleep(Duration::from_millis(250)).await;

    let name = DestinationName::new("retinue", ["reliable-oracle"]);
    endpoint.register_reliable(name.clone(), b"reliable-oracle");
    for _ in 0..5 {
        endpoint.announce(&name, b"reliable-oracle");
        tokio::time::sleep(Duration::from_millis(600)).await;
    }

    for round in 0..2 {
        println!("WAITING_LINK {round}");
        let mut link =
            tokio::time::timeout(Duration::from_secs(25), endpoint.accept_reliable()).await??;
        println!("LINK {round} {}", link.link_id());

        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), link.read_to_end(&mut received)).await??;
        println!("READ_EOF {round} {}", received.len());
        let expected = if round == 0 {
            let mut bytes = PREFIX.to_vec();
            bytes.extend_from_slice(&[b'Z'; 320]);
            bytes
        } else {
            let mut bytes = COMBINED_PREFIX.to_vec();
            bytes.extend_from_slice(&[b'Y'; 8192]);
            bytes
        };
        if received != expected {
            return Err(format!(
                "RNS round {round} bytes mismatch: received {} bytes",
                received.len()
            )
            .into());
        }
        println!("RECV_OK {round}");

        let reply = if round == 0 {
            let mut bytes = REPLY_PREFIX.to_vec();
            bytes.extend_from_slice(&[b'Q'; 640]);
            bytes
        } else {
            COMBINED_REPLY.to_vec()
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            link.write_all(&reply).await?;
            link.shutdown().await
        })
        .await??;
        println!("SENT_EOF {round} {}", reply.len());
    }
    tokio::time::timeout(
        Duration::from_secs(15),
        endpoint.shutdown(Duration::from_secs(5)),
    )
    .await?;
    println!("DONE");
    Ok(())
}
