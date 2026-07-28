//! Retinue half of the live IFAC interoperability gate.
//!
//! Driven by `oracle/interop_ifac.py`. The stable output lines are its receipt.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use retinue::Ifac;
use retinue::announce::{self, Announce, RAND_HASH_LEN};
use retinue::destination::DestinationName;
use retinue::identity::PrivateIdentity;
use retinue::iface::tcp::{RecvError, TcpInterfaceListener};
use retinue::packet::PacketType;

const RETINUE_SEED: [u8; 64] = [0x21; 64];

fn rand_hash() -> [u8; RAND_HASH_LEN] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after 1970")
        .as_nanos()
        .to_le_bytes();
    let mut out = [0_u8; RAND_HASH_LEN];
    out.copy_from_slice(&nanos[..RAND_HASH_LEN]);
    out
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let access = Ifac::new(Some("retinue-ifac-interop"), Some("mixed-runtime"), 8)?;
    let listener = TcpInterfaceListener::bind_with_ifac("127.0.0.1:0".parse()?, access).await?;
    println!("LISTENING {}", listener.local_addr()?.port());

    let mut interface = listener.accept().await?;
    // RNS 1.4.0 completes its TCPClientInterface IFAC fields just after connect.
    tokio::time::sleep(Duration::from_millis(250)).await;
    println!("ACCEPTED {}", interface.peer_addr()?);

    let identity = PrivateIdentity::from_secret_bytes(&RETINUE_SEED);
    let name = DestinationName::new("retinue", ["interop_ifac"]);
    let packet = announce::build(
        &identity,
        name.name_hash(),
        &rand_hash(),
        None,
        b"hello-from-retinue-ifac",
    );
    let destination = name.destination_hash(identity.public());
    interface.send(&packet).await?;
    println!("SENT_IFAC_ANNOUNCE {destination}");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            println!("TIMEOUT waiting for an IFAC announce from RNS");
            return Ok(());
        }
        match tokio::time::timeout(remaining, interface.recv()).await {
            Err(_) => {
                println!("TIMEOUT waiting for an IFAC announce from RNS");
                return Ok(());
            }
            Ok(Err(RecvError::Io(error))) => {
                println!("IO_ERROR {error}");
                return Ok(());
            }
            Ok(Err(RecvError::Wire(error))) => {
                println!("SKIPPED invalid IFAC frame: {error}");
            }
            Ok(Ok(packet)) if packet.packet_type != PacketType::Announce => {
                println!("SKIPPED non-announce packet type {:?}", packet.packet_type);
            }
            Ok(Ok(packet)) => match Announce::decode(&packet) {
                Ok(announce) => {
                    println!("RECV_IFAC_ANNOUNCE {}", announce.destination);
                    println!("VALIDATED_RNS_IFAC_ANNOUNCE");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    return Ok(());
                }
                Err(error) => {
                    println!("REJECTED_RNS_IFAC_ANNOUNCE {error}");
                    return Ok(());
                }
            },
        }
    }
}
