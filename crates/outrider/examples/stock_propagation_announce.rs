//! Black-box oracle receiver for a stock LXMF propagation-node announce.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x28; 64],
    )));
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;
    println!("LISTENING {}", address.port());

    let propagation_name = DestinationName::new("lxmf", ["propagation"]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("timed out waiting for stock propagation announce".into());
        }
        let announce = tokio::time::timeout(remaining, endpoint.next_announcement()).await??;
        if propagation_name.destination_hash(&announce.identity) == announce.destination {
            println!("PROPAGATION_DESTINATION {}", announce.destination);
            println!(
                "PROPAGATION_IDENTITY {}",
                hex::encode(announce.identity.to_public_bytes())
            );
            println!("PROPAGATION_APP_DATA {}", hex::encode(&announce.app_data));
            let decoded = rmpv::decode::read_value(&mut Cursor::new(&announce.app_data))?;
            println!("PROPAGATION_APP_DATA_DECODED {decoded:?}");
            break;
        }
    }

    endpoint.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
