//! Live oracle peer: stock RNS encrypts to a ratchet this endpoint has since rotated away
//! from, and the retained epoch still decrypts it.
//!
//! Driven over stdin by `oracle/interop_ratchet_rotation.py`. Each `ANNOUNCE` line
//! announces the destination, rotating first once the 2 s interval has passed, and prints
//! the advertised ratchet id. The persistence hook prints each snapshot's current id, so
//! the driver can check that a ratchet is persisted before it is advertised.

use std::io::{self, BufRead};
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::ratchet::{RatchetPolicy, RatchetStore};

const SEED: [u8; 64] = [0x73; 64];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let identity = PrivateIdentity::from_secret_bytes(&SEED);
    let endpoint = Endpoint::new(identity.clone());
    let address = endpoint.listen_tcp("127.0.0.1:0".parse()?).await?;

    let policy = RatchetPolicy {
        rotation_interval: Duration::from_secs(2),
        ..RatchetPolicy::default()
    };
    let owner = *identity.public();
    let hook_policy = policy.clone();
    endpoint.set_ratchet_persistence(move |_, snapshot| {
        let (store, _) = RatchetStore::restore(hook_policy.clone(), snapshot, &owner, 0.0)
            .map_err(io::Error::other)?;
        println!("PERSISTED {}", store.current_id().expect("persisted epoch"));
        Ok(())
    });

    let name = DestinationName::new("retinue", ["ratchet", "rotation"]);
    endpoint.register_resource_with_ratchets(name.clone(), b"", RatchetStore::new(policy)?)?;
    println!("LISTENING {}", address.port());
    println!("DESTINATION {}", name.destination_hash(identity.public()));

    let (commands, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if commands.send(line).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            command = command_rx.recv() => match command.as_deref() {
                Some("ANNOUNCE") => {
                    endpoint.announce(&name, b"");
                    let current = endpoint.current_ratchet_id(&name).expect("ratcheted");
                    println!("ANNOUNCED {current}");
                }
                _ => break,
            },
            single = endpoint.accept_single() => {
                let single = single?;
                println!(
                    "RECEIVED {} {}",
                    String::from_utf8_lossy(&single.data),
                    single.ratchet_id.expect("ratchet authenticated"),
                );
            }
        }
    }
    endpoint.shutdown(Duration::from_secs(1)).await;
    Ok(())
}
